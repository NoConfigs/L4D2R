//! Retail SurvivorLegsRetreat: a survivor bot backs away from a dangerous
//! threat it can see inside that threat's danger range, still shooting it,
//! until it is 1.5x the range away ("Retreated a safe distance") or has lost
//! sight of it for a second ("Retreated out of sight of threat").
//! Ranges are FUN_1038eb50 (server.dll build 10097); the end checks are
//! FUN_103bd940 (DAT_105d4b38 = 1.5).
use bevy::prelude::*;

use crate::collision::UNIT;

/// Danger ranges (units): Tank, enraged Witch, Boomer
/// (z_exploding_outer_radius 200 x 1.1) and the other specials.
pub const TANK_RANGE: f32 = 450.;
pub const WITCH_RANGE: f32 = 300.;
pub const BOOMER_RANGE: f32 = 220.;
pub const SPECIAL_RANGE: f32 = 125.;
/// The retreat ends past this multiple of the danger range...
pub const SAFE_FACTOR: f32 = 1.5;
/// ...or after this long without seeing the threat.
pub const OUT_OF_SIGHT: f32 = 1.;

#[derive(Clone, Copy, Debug)]
pub struct Threat {
    pub id: Entity,
    pub position: Vec3,
    /// Danger range in metres.
    pub range: f32,
}

/// Danger range (metres) of an infected by model, or None when it is no
/// reason to back off. A special holding a survivor is not one: bots close
/// in to shove or shoot it off. A Witch counts only once startled.
pub fn danger_range(model: &str, pinning: bool, calm_witch: bool) -> Option<f32> {
    use crate::special_infected::SpecialClass as C;
    let units = match C::from_model(model) {
        Some(C::Tank) => TANK_RANGE,
        Some(_) if pinning => return None,
        Some(C::Boomer) => BOOMER_RANGE,
        Some(_) => SPECIAL_RANGE,
        None if model.contains("witch") && !calm_witch => WITCH_RANGE,
        None => return None,
    };
    Some(units * UNIT)
}

/// Per-bot retreat: (threat, seconds out of sight).
#[derive(Default)]
pub struct Retreats(Vec<Option<(Entity, f32)>>);

impl Retreats {
    pub fn reset(&mut self, index: usize) {
        if let Some(slot) = self.0.get_mut(index) {
            *slot = None;
        }
    }
    pub fn active(&self, index: usize) -> bool {
        self.0.get(index).is_some_and(|s| s.is_some())
    }
    /// Horizontal direction bot `index` at `at` should back away in this
    /// tick, if it is retreating. `visible` tests sight to a threat position.
    pub fn update(
        &mut self,
        index: usize,
        at: Vec3,
        threats: &[Threat],
        dt: f32,
        visible: impl Fn(Vec3) -> bool,
    ) -> Option<Vec3> {
        if self.0.len() <= index {
            self.0.resize(index + 1, None);
        }
        let current = self.0[index].and_then(|(id, _)| threats.iter().find(|t| t.id == id).copied());
        if self.0[index].is_some() && current.is_none() {
            // "Threat is gone".
            self.0[index] = None;
        }
        // Start on (or switch to a more dangerous) visible threat inside
        // its danger range.
        let worse = threats
            .iter()
            .filter(|t| t.position.distance(at) < t.range)
            .filter(|t| current.is_none_or(|c| t.range > c.range))
            .filter(|t| visible(t.position))
            .max_by(|a, b| a.range.total_cmp(&b.range))
            .copied();
        let threat = match (worse, current) {
            (Some(t), _) => {
                self.0[index] = Some((t.id, 0.));
                t
            }
            (None, Some(t)) => t,
            (None, None) => return None,
        };
        let away = (at - threat.position) * Vec3::new(1., 0., 1.);
        if away.length() > threat.range * SAFE_FACTOR {
            self.0[index] = None;
            return None;
        }
        let slot = self.0[index].as_mut()?;
        if visible(threat.position) {
            slot.1 = 0.;
        } else {
            slot.1 += dt;
            if slot.1 > OUT_OF_SIGHT {
                self.0[index] = None;
                return None;
            }
        }
        Some(away.normalize_or(Vec3::X))
    }
}

/// Retreat direction bent sideways around teammates standing in the way
/// (within 1.5 m ahead), so a bot backs past them instead of into them.
pub fn around(away: Vec3, at: Vec3, mates: &[Vec3]) -> Vec3 {
    let side = away.cross(Vec3::Y);
    let mut bent = away;
    for p in mates {
        let d = (*p - at) * Vec3::new(1., 0., 1.);
        let len = d.length();
        if len < 1.5 && d.dot(away) > 0. {
            let sign = if d.dot(side) > 0. { -1. } else { 1. };
            bent += side * sign * (1. - len / 1.5) * 2.;
        }
    }
    bent.normalize_or(away)
}


/// Retail SurvivorLegsCoverFriendInCombat ("A friend is in combat and I'm
/// not - moving to assist"; legs selector FUN_103c06a0, Update FUN_103bee80).
/// A bot that has been calm for a second heads for the nearest teammate in
/// combat until it is "in position" (FUN_1038eaa0): within 300 units, the
/// "how close a friend needs to be to feel safe" cvar, with a clear line of
/// sight. While it can see that friend it looks where the friend looks
/// (eye + forward x 1000 units, "Supporting friend in combat").
/// "In combat" (FUN_103089a0(1)) is being held by a special, having shot
/// within the last 3 s, or having been hurt within the last 1.5 s.
pub const ASSIST_RANGE: f32 = 300. * UNIT;
/// The supporting look point lies this far along the friend's view.
pub const SUPPORT_LOOK: f32 = 1000. * UNIT;
const FIRED_WINDOW: f32 = 3.;
const HURT_WINDOW: f32 = 1.5;
/// FUN_1038e9d0: the bot itself must have been calm this long.
pub const ASSIST_CALM: f32 = 1.;

#[derive(Clone, Copy, Default)]
struct CombatTrack {
    shot: u64,
    damage: u64,
    shot_at: Option<f32>,
    hurt_at: Option<f32>,
}
#[derive(Default)]
pub struct Assist {
    tracks: std::collections::HashMap<usize, CombatTrack>,
    /// The friend (squad index, or `bot_look::PLAYER`) each bot is assisting.
    targets: Vec<Option<usize>>,
}
impl Assist {
    /// Records a survivor's shot and damage counters this tick.
    pub fn observe(&mut self, key: usize, shot_serial: u64, damage_serial: u64, now: f32) {
        let track = self.tracks.entry(key).or_insert(CombatTrack {
            shot: shot_serial,
            damage: damage_serial,
            ..default()
        });
        if track.shot != shot_serial {
            track.shot = shot_serial;
            track.shot_at = Some(now);
        }
        if track.damage != damage_serial {
            track.damage = damage_serial;
            track.hurt_at = Some(now);
        }
    }
    pub fn in_combat(&self, key: usize, pinned: bool, now: f32) -> bool {
        pinned
            || self.tracks.get(&key).is_some_and(|t| {
                t.shot_at.is_some_and(|at| now - at < FIRED_WINDOW)
                    || t.hurt_at.is_some_and(|at| now - at < HURT_WINDOW)
            })
    }
    /// Picks the friend bot `index` should assist: the nearest one in combat
    /// that it is not yet in position for. `friends` are (key, position,
    /// in combat, in position).
    pub fn choose(
        &mut self,
        index: usize,
        position: Vec3,
        friends: &[(usize, Vec3, bool, bool)],
    ) -> Option<Vec3> {
        if self.targets.len() <= index {
            self.targets.resize(index + 1, None);
        }
        let pick = friends
            .iter()
            .filter(|(key, _, combat, in_position)| *key != index && *combat && !*in_position)
            .min_by(|a, b| a.1.distance_squared(position).total_cmp(&b.1.distance_squared(position)));
        self.targets[index] = pick.map(|f| f.0);
        pick.map(|f| f.1)
    }
    /// The friend bot `index` is moving to assist.
    pub fn target(&self, index: usize) -> Option<usize> {
        self.targets.get(index).copied().flatten()
    }
    pub fn forget(&mut self, index: usize) {
        if let Some(t) = self.targets.get_mut(index) {
            *t = None;
        }
    }
}

/// Retail SurvivorLegsBattleStations ("I've entered a battlefield"; legs
/// selector FUN_103c06a0, Update FUN_103bb810, station choice FUN_103bb500).
/// - Every RandomFloat(3, 5) s, while no Tank is alive, a bot whose human is
///   in a BATTLEFIELD/FINALE area with a BATTLESTATION area within 500 units
///   (flooding through connected battlefield/finale areas) takes up stations.
/// - Every RandomFloat(0.5, 2) s, once the human has held position for
///   sb_battlestation_human_hold_time (4 s), it picks the station nearest the
///   human within sb_max_battlestation_range_from_human (750) that the
///   human's area can potentially see and that is not full (an area holds
///   size x 0.000178 survivors, at least one). Its current station is kept
///   unless another is under half its squared distance.
/// - It walks there while the human stays within 750 of it; once there it
///   drops it when the human is beyond the give-up range (1500).
/// - It stops when a Tank is alive ("A Tank is coming") or no station is
///   found ("No practical battlestation found").
/// Not modelled: the flow-distance entry check, the finale/rescue vehicle
/// transitions and the per-area blocked markers.
pub const STATION_ENTER_RANGE: f32 = 500. * UNIT;
pub const STATION_RANGE: f32 = 750. * UNIT;
pub const STATION_GIVE_UP: f32 = 1500. * UNIT;
pub const HUMAN_HOLD_TIME: f32 = 4.;
const AREA_CAPACITY: f32 = 0.000_177_777_78;

#[derive(Clone, Copy, Default)]
struct Station {
    active: bool,
    area: Option<usize>,
    choose_at: f32,
    enter_at: f32,
}
pub struct BattleStations {
    bots: Vec<Station>,
    rng: l4d2r_game_core::random::UniformRandomStream,
}
impl Default for BattleStations {
    fn default() -> Self {
        Self {
            bots: vec![],
            rng: l4d2r_game_core::random::UniformRandomStream::new(0xba77),
        }
    }
}
/// What the team looks like this tick, for station choices.
pub struct StationWorld<'a> {
    pub now: f32,
    /// The human's feet, or None when there is no living human.
    pub human: Option<Vec3>,
    /// How long the human has held position.
    pub human_held: f32,
    pub tank_alive: bool,
    /// Feet of every living survivor (bots and the human).
    pub survivors: &'a [Vec3],
}
impl BattleStations {
    fn bot(&mut self, index: usize) -> &mut Station {
        if self.bots.len() <= index {
            self.bots.resize(index + 1, Station::default());
        }
        &mut self.bots[index]
    }
    pub fn forget(&mut self, index: usize) {
        *self.bot(index) = Station::default();
    }
    fn full(areas: &impl crate::bot_look::Areas, area: usize, me: Vec3, world: &StationWorld) -> bool {
        let Some(info) = areas.info(area) else {
            return true;
        };
        let mut occupants = world
            .survivors
            .iter()
            .filter(|p| areas.area_at(**p) == Some(area))
            .count() as i32;
        if areas.area_at(me) == Some(area) {
            occupants -= 1;
        }
        let capacity = ((info.size * AREA_CAPACITY) as i32).max(1);
        capacity <= occupants
    }
    /// Station goal for bot `index` at `position`, or None when it is not
    /// taking up a station.
    pub fn goal(
        &mut self,
        index: usize,
        position: Vec3,
        areas: &impl crate::bot_look::Areas,
        world: &StationWorld,
    ) -> Option<Vec3> {
        let now = world.now;
        let Some(human) = world.human else {
            self.forget(index);
            return None;
        };
        if world.tank_alive {
            self.forget(index);
            return None;
        }
        if !self.bot(index).active {
            if now < self.bot(index).enter_at {
                return None;
            }
            let wait = self.rng.random_float(3., 5.);
            self.bot(index).enter_at = now + wait;
            if areas.battlestations(human, Some(STATION_ENTER_RANGE)).is_empty() {
                return None;
            }
            *self.bot(index) = Station { active: true, area: None, choose_at: now, enter_at: now + wait };
        }
        if now >= self.bot(index).choose_at {
            let wait = self.rng.random_float(0.5, 2.);
            self.bot(index).choose_at = now + wait;
            if world.human_held >= HUMAN_HOLD_TIME {
                let human_area = areas.area_at(human);
                let seen = |a: usize| human_area.is_some_and(|h| areas.visible(h, a, crate::bot_look::POTENTIALLY_VISIBLE));
                let centre = |a: usize| areas.info(a).map(|i| i.center);
                let mut best = None;
                let mut limit = STATION_RANGE * STATION_RANGE;
                if let Some(current) = self.bot(index).area {
                    if let Some(c) = centre(current).filter(|c| c.distance(human) <= STATION_RANGE && seen(current)) {
                        best = Some(current);
                        limit = c.distance_squared(human) * 0.5;
                    }
                }
                for s in areas.battlestations(human, None) {
                    let Some(c) = centre(s) else { continue };
                    let d2 = c.distance_squared(human);
                    if d2 < limit && seen(s) && !Self::full(areas, s, position, world) {
                        best = Some(s);
                        limit = d2;
                    }
                }
                self.bot(index).area = best;
            }
            if self.bot(index).area.is_none() && world.human_held >= HUMAN_HOLD_TIME {
                // "No practical battlestation found".
                let enter = self.bot(index).enter_at;
                *self.bot(index) = Station { enter_at: enter, ..Station::default() };
                return None;
            }
        }
        let station = self.bot(index).area?;
        let centre = areas.info(station)?.center;
        let arrived = areas.area_at(position) == Some(station);
        let range = if arrived { STATION_GIVE_UP } else { STATION_RANGE };
        if centre.distance(human) > range || (!arrived && Self::full(areas, station, position, world)) {
            self.bot(index).area = None;
            return None;
        }
        Some(centre)
    }
}

/// Retail SurvivorLegsRegroup for "I've lost sight of the team" (legs
/// selector FUN_103c06a0, last-seen times FUN_103a5450, target FUN_103a6e90,
/// Update FUN_103be650). A bot that has not seen any teammate for 3 s heads
/// for the nearest reachable one, and stops ("Near our team again") once it
/// is within 200 units (the 0x10865064 range cvar) and has seen a teammate
/// in the last 0.5 s.
/// Not modelled: the per-bot 10-unit range stagger, path-length target
/// choice (straight distance is used) and vision field of view (line of
/// sight alone counts as seeing a teammate).
pub const LOST_SIGHT: f32 = 3.;
pub const REGROUP_RANGE: f32 = 200. * UNIT;
const REGROUPED_SEEN: f32 = 0.5;

#[derive(Clone, Copy, Default)]
struct Regroup {
    unseen: f32,
    target: Option<usize>,
}
#[derive(Default)]
pub struct Regroups(Vec<Regroup>);
impl Regroups {
    fn bot(&mut self, index: usize) -> &mut Regroup {
        if self.0.len() <= index {
            self.0.resize(index + 1, Regroup::default());
        }
        &mut self.0[index]
    }
    pub fn forget(&mut self, index: usize) {
        *self.bot(index) = Regroup::default();
    }
    /// `teammates` are (key, feet, visible from this bot). Returns where the
    /// bot regroups to, or None.
    pub fn update(
        &mut self,
        index: usize,
        position: Vec3,
        teammates: &[(usize, Vec3, bool)],
        dt: f32,
    ) -> Option<Vec3> {
        let bot = self.bot(index);
        if teammates.iter().any(|t| t.0 != index && t.2) {
            bot.unseen = 0.;
        } else {
            bot.unseen += dt;
        }
        let others = || teammates.iter().filter(|t| t.0 != index);
        if bot.target.is_none() && bot.unseen > LOST_SIGHT {
            bot.target = others()
                .min_by(|a, b| a.1.distance_squared(position).total_cmp(&b.1.distance_squared(position)))
                .map(|t| t.0);
        }
        let key = bot.target?;
        let Some(&(_, at, _)) = others().find(|t| t.0 == key) else {
            // "No reachable teammates left alive" for this one; pick again.
            bot.target = None;
            return None;
        };
        if at.distance(position) <= REGROUP_RANGE && bot.unseen < REGROUPED_SEEN {
            bot.target = None;
            return None;
        }
        Some(at)
    }
    pub fn active(&self, index: usize) -> bool {
        self.0.get(index).is_some_and(|r| r.target.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bots_regroup_after_losing_sight_of_the_team_for_three_seconds() {
        let mut r = Regroups::default();
        let me = Vec3::ZERO;
        let far = Vec3::new(12., 0., 0.);
        let hidden = [(9, far, false)];
        let mut t = 0.;
        while t < 3.0 {
            assert_eq!(r.update(0, me, &hidden, 0.1), None, "still within the 3 s at {t}");
            t += 0.1;
        }
        assert_eq!(r.update(0, me, &hidden, 0.2), Some(far));
        // Seeing the teammate again from inside 200 units ends it.
        let near = Vec3::new(4., 0., 0.);
        assert_eq!(r.update(0, me, &[(9, near, true)], 0.1), None);
        assert!(!r.active(0));
    }
    struct Field;
    impl crate::bot_look::Areas for Field {
        fn area_at(&self, p: Vec3) -> Option<usize> {
            Some(((p.x / 10.).floor().max(0.) as usize).min(3))
        }
        fn count(&self) -> usize {
            4
        }
        fn info(&self, a: usize) -> Option<crate::bot_look::AreaInfo> {
            Some(crate::bot_look::AreaInfo {
                center: Vec3::new(a as f32 * 10. + 5., 0., 0.),
                size: 100. * 100.,
                attributes: if a == 1 || a == 3 { 0x120 } else { 0x100 },
            })
        }
        fn visible_areas(&self, _: usize, _: u8) -> Vec<usize> {
            vec![0, 1, 2, 3]
        }
        fn visible(&self, _: usize, _: usize, _: u8) -> bool {
            true
        }
        fn battlestations(&self, _: Vec3, range: Option<f32>) -> Vec<usize> {
            // Area 1 is 10 m from the human, area 3 is 30 m (beyond 750 units).
            if range.is_some_and(|r| r < 10.) { vec![] } else { vec![1, 3] }
        }
    }
    #[test]
    fn bots_take_the_station_nearest_a_human_holding_position() {
        let mut st = BattleStations::default();
        let feet = [Vec3::new(5., 0., 0.)];
        let mut world = StationWorld { now: 0., human: Some(feet[0]), human_held: 0., tank_alive: false, survivors: &feet };
        let bot = Vec3::new(25., 0., 0.);
        assert_eq!(st.goal(0, bot, &Field, &world), None, "the human has not held position yet");
        world.now = 2.;
        world.human_held = 4.;
        assert_eq!(st.goal(0, bot, &Field, &world), Some(Vec3::new(15., 0., 0.)));
        world.now = 2.1;
        world.tank_alive = true;
        assert_eq!(st.goal(0, bot, &Field, &world), None, "a Tank is coming");
    }
    #[test]
    fn calm_bots_assist_the_nearest_friend_in_combat_until_in_position() {
        let mut a = Assist::default();
        a.observe(7, 0, 0, 0.);
        assert!(!a.in_combat(7, false, 0.5));
        a.observe(7, 1, 0, 1.);
        assert!(a.in_combat(7, false, 3.9) && !a.in_combat(7, false, 4.1), "shot within 3 s");
        a.observe(7, 1, 1, 5.);
        assert!(a.in_combat(7, false, 6.4) && !a.in_combat(7, false, 6.6), "hurt within 1.5 s");
        assert!(a.in_combat(7, true, 100.), "held by a special");
        let near = Vec3::new(5., 0., 0.);
        let far = Vec3::new(20., 0., 0.);
        let friends = [(1, far, true, false), (2, near, true, false), (3, Vec3::ZERO, false, false)];
        assert_eq!(a.choose(0, Vec3::ZERO, &friends), Some(near));
        assert_eq!(a.target(0), Some(2));
        let friends = [(2, near, true, true)];
        assert_eq!(a.choose(0, Vec3::ZERO, &friends), None, "already in position");
        assert_eq!(a.target(0), None);
    }

    fn tank(at: Vec3) -> Threat {
        Threat { id: Entity::from_bits(7), position: at, range: TANK_RANGE * UNIT }
    }

    #[test]
    fn danger_ranges_follow_retail_threat_classes() {
        use crate::special_infected::SpecialClass as C;
        assert_eq!(danger_range(C::Tank.model(), false, false), Some(450. * UNIT));
        assert_eq!(danger_range(C::Boomer.model(), false, false), Some(220. * UNIT));
        assert_eq!(danger_range(C::Hunter.model(), false, false), Some(125. * UNIT));
        // A special pinning someone is left to the rescue, a calm witch alone.
        assert_eq!(danger_range(C::Hunter.model(), true, false), None);
        assert_eq!(danger_range("models/infected/witch.mdl", false, true), None);
        assert_eq!(danger_range("models/infected/witch.mdl", false, false), Some(300. * UNIT));
        assert_eq!(danger_range("models/infected/common_male01.mdl", false, false), None);
    }

    #[test]
    fn bots_back_off_a_tank_until_a_safe_distance_or_out_of_sight() {
        let mut r = Retreats::default();
        let threat = [tank(Vec3::ZERO)];
        // Outside the danger range: no retreat.
        assert!(r.update(0, Vec3::X * 12., &threat, 0.1, |_| true).is_none());
        // Inside it: back straight away from the Tank.
        let away = r.update(0, Vec3::X * 8., &threat, 0.1, |_| true).unwrap();
        assert!(away.dot(Vec3::X) > 0.99);
        // Keeps retreating between the danger range and 1.5x it...
        assert!(r.update(0, Vec3::X * 14., &threat, 0.1, |_| true).is_some());
        // ...and stops past 1.5x (450 * 1.5 = 675 units = 17.1 m).
        assert!(r.update(0, Vec3::X * 17.5, &threat, 0.1, |_| true).is_none());
        // Out of sight for over a second also ends it.
        assert!(r.update(1, Vec3::X * 8., &threat, 0.1, |_| true).is_some());
        assert!(r.update(1, Vec3::X * 12., &threat, 0.6, |_| false).is_some());
        assert!(r.update(1, Vec3::X * 12., &threat, 0.6, |_| false).is_none());
        // An unseen threat never starts a retreat.
        assert!(r.update(2, Vec3::X * 8., &threat, 0.1, |_| false).is_none());
        // A dead threat ends it.
        assert!(r.update(0, Vec3::X * 8., &threat, 0.1, |_| true).is_some());
        assert!(r.update(0, Vec3::X * 8., &[], 0.1, |_| true).is_none());
    }

    #[test]
    fn retreats_sidestep_a_teammate_in_the_way() {
        let bent = around(Vec3::X, Vec3::ZERO, &[Vec3::X * 0.8]);
        assert!(bent.z.abs() > 0.5 && bent.x > 0.);
        // Off to one side: step away from that side.
        let bent = around(Vec3::X, Vec3::ZERO, &[Vec3::new(0.8, 0., 0.3)]);
        assert!(bent.z < -0.3);
        // Behind or far away: unchanged.
        assert_eq!(around(Vec3::X, Vec3::ZERO, &[Vec3::X * -0.8, Vec3::X * 3.]), Vec3::X);
    }
    #[test]
    fn a_more_dangerous_threat_takes_over_the_retreat() {
        let mut r = Retreats::default();
        let hunter = Threat { id: Entity::from_bits(3), position: Vec3::X * 2., range: SPECIAL_RANGE * UNIT };
        let at = Vec3::new(2., 0., 2.);
        let away = r.update(0, at, &[hunter], 0.1, |_| true).unwrap();
        assert!(away.dot(Vec3::Z) > 0.99);
        let big = tank(Vec3::new(2., 0., 10.));
        let away = r.update(0, at, &[hunter, big], 0.1, |_| true).unwrap();
        assert!(away.dot(Vec3::NEG_Z) > 0.99);
    }
}
