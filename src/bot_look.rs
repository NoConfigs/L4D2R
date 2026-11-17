//! Where survivor bots look when nothing threatens them. Retail SurvivorBot
//! (server 10397410 / 1038f030): it first clears newly revealed areas
//! (`Clearing` below); with none queued, a moving bot looks ahead;
//! standing, it "looks around" every RandomFloat(3, 5) s, 33% of the time
//! at a random friend it can see ("Looking around at friend"), otherwise at
//! a visible spot 300 units out, RandomFloat(0.5, 2) right-vectors to a
//! random side of its current view and RandomFloat(-0.5, 0.5) up
//! ("Looking around at random spot"). The head turns at
//! sb_normal_saccade_speed (350 deg/s).
use bevy::prelude::*;
use l4d2r_game_core::random::UniformRandomStream;

const UNIT: f32 = 0.0254;
/// sb_normal_saccade_speed (deg/s).
pub const SACCADE_SPEED: f32 = 350.;
const LOOK_DISTANCE: f32 = 300. * UNIT;

#[derive(Clone, Copy)]
enum Target {
    /// A friend's eye, by friend key (squad index, or `PLAYER`).
    Friend(usize),
    Point(Vec3),
}
pub const PLAYER: usize = usize::MAX;

#[derive(Default, Clone, Copy)]
struct Look {
    next: f32,
    target: Option<Target>,
    last: Option<Vec3>,
}

pub struct BotLook {
    looks: Vec<Look>,
    rng: UniformRandomStream,
}
impl Default for BotLook {
    fn default() -> Self {
        Self { looks: vec![], rng: UniformRandomStream::new(0x5b07) }
    }
}

impl BotLook {
    /// Wanted look direction for an unthreatened bot. `friends` are the
    /// (key, eye) of other living survivors; `sees` tests line of sight.
    #[allow(clippy::too_many_arguments)]
    pub fn wanted(
        &mut self,
        index: usize,
        now: f32,
        position: Vec3,
        eye: Vec3,
        aim: Vec3,
        friends: &[(usize, Vec3)],
        sees: impl Fn(Vec3, Vec3) -> bool,
    ) -> Vec3 {
        if self.looks.len() <= index {
            self.looks.resize(index + 1, Look::default());
        }
        let moved = self.looks[index].last.map(|l| (position - l) * Vec3::new(1., 0., 1.));
        self.looks[index].last = Some(position);
        // Moving: look where we are going (clearing the areas ahead).
        if let Some(step) = moved.filter(|m| m.length() > 0.01) {
            self.looks[index].target = None;
            self.looks[index].next = now;
            return step.normalize();
        }
        if now >= self.looks[index].next {
            self.looks[index].next = now + self.rng.random_float(3., 5.);
            self.looks[index].target = None;
            if self.rng.random_float(0., 100.) < 33. {
                let visible: Vec<_> = friends.iter().filter(|(_, f)| sees(eye, *f)).collect();
                if !visible.is_empty() {
                    // Retail makes up to four random picks from its recently
                    // seen friends; a single pick among visible ones matches.
                    let pick = self.rng.random_int(0, visible.len() as i32 - 1) as usize;
                    self.looks[index].target = Some(Target::Friend(visible[pick].0));
                }
            }
            if self.looks[index].target.is_none() {
                let forward = (aim * Vec3::new(1., 0., 1.)).normalize_or(Vec3::X);
                let right = Vec3::new(-forward.z, 0., forward.x);
                let side = if self.rng.random_int(0, 100) < 50 { -1. } else { 1. };
                let up = self.rng.random_float(-0.5, 0.5);
                let across = self.rng.random_float(0.5, 2.) * side;
                let direction = (forward + right * across + Vec3::Y * up).normalize();
                let spot = eye + direction * LOOK_DISTANCE;
                if sees(eye, spot) {
                    self.looks[index].target = Some(Target::Point(spot));
                }
            }
        }
        let target = match self.looks[index].target {
            Some(Target::Friend(key)) => friends.iter().find(|(k, _)| *k == key).map(|f| f.1),
            Some(Target::Point(p)) => Some(p),
            None => None,
        };
        target.map_or(aim, |t| (t - eye).normalize_or(aim))
    }
    pub fn forget(&mut self, index: usize) {
        if let Some(look) = self.looks.get_mut(index) {
            *look = Look::default();
        }
    }
}

/// Turns `current` toward `wanted` at most `speed` degrees per second.
pub fn turn(current: Vec3, wanted: Vec3, speed: f32, dt: f32) -> Vec3 {
    let (Some(a), Some(b)) = (current.try_normalize(), wanted.try_normalize()) else {
        return wanted.normalize_or(current);
    };
    let angle = a.angle_between(b);
    let step = (speed * dt).to_radians();
    if angle <= step || angle < 1e-4 {
        return b;
    }
    a.slerp(b, step / angle).normalize()
}


/// NAV area data needed for area clearing (implemented by Navigation).
pub trait Areas {
    fn area_at(&self, position: Vec3) -> Option<usize>;
    fn count(&self) -> usize;
    fn info(&self, area: usize) -> Option<AreaInfo>;
    /// Areas in `from`'s visibility set whose attributes match `mask`
    /// (1 partially, 2 completely visible), inherited set included.
    fn visible_areas(&self, from: usize, mask: u8) -> Vec<usize>;
    fn visible(&self, from: usize, to: usize, mask: u8) -> bool;
    /// BATTLESTATION (0x20) areas reached from the area under `position`
    /// through connected BATTLEFIELD/FINALE (0x140) areas, within `range`
    /// metres of `position` when given (server FUN_103bb0a0 / 103bb020).
    fn battlestations(&self, position: Vec3, range: Option<f32>) -> Vec<usize>;
}
#[derive(Clone, Copy, Debug)]
pub struct AreaInfo {
    pub center: Vec3,
    /// Source units squared.
    pub size: f32,
    /// TerrorNavArea spawn attributes.
    pub attributes: u32,
}
pub const COMPLETELY_VISIBLE: u8 = 2;
pub const POTENTIALLY_VISIBLE: u8 = 3;
const PLAYER_START: u32 = 0x80;
const NOT_CLEARABLE: u32 = 0x400;
const CHECKPOINT: u32 = 0x800;
const OBSCURED: u32 = 0x1000;
/// The look point sits 0.75 x the 72-unit hull above the area centre.
const CLEAR_LOOK_HEIGHT: f32 = 0.75 * 72. * UNIT;
/// An area this close to the eye counts as cleared.
const CLEAR_REACHED: f32 = 100. * UNIT;
/// ...as does one the bot faces (dot > 0.8) with a steady head for
/// sqrt(area) * 0.005 s (at most 1 s).
const CLEAR_FACING: f32 = 0.8;
const CLEAR_PRUNE: f32 = 1.;

/// Retail SurvivorBot area clearing ("Clearing an area", server 10397410).
/// Each survivor marks every NAV area completely visible from where it
/// stands as cleared (1030b720 -> 102eded0; start and checkpoint areas are
/// always cleared, NOT_CLEARABLE ones never). When a bot steps into a new
/// area it queues the completely visible areas no survivor has cleared yet
/// (OnNavAreaChanged 1039ce80, filter 10398570, skipping NOT_CLEARABLE,
/// CHECKPOINT and OBSCURED areas). A calm bot looks at the newest queued
/// area; each second it drops areas no longer potentially visible, and an
/// area leaves the queue once reached or looked at long enough.
#[derive(Default)]
pub struct Clearing {
    cleared: Vec<bool>,
    key: Option<(u64, usize)>,
    bots: Vec<ClearBot>,
    /// The local survivor's own cleared bits and area (retail keeps one
    /// bit per survivor, TerrorNavArea +0x13d).
    player_cleared: Vec<bool>,
    player_area: Option<usize>,
    /// Newly exposed area (Source units squared) from the player's last
    /// area change, taken by `take_revealed`.
    revealed: f32,
}
#[derive(Default, Clone)]
struct ClearBot {
    area: Option<usize>,
    queue: Vec<usize>,
    prune_at: f32,
    steady: f32,
    last_aim: Vec3,
}
impl Clearing {
    /// Starts over for a new map or a restart.
    pub fn sync(&mut self, generation: u64, count: usize) {
        if self.key != Some((generation, count)) {
            *self = Self {
                cleared: vec![false; count],
                key: Some((generation, count)),
                player_cleared: vec![false; count],
                ..default()
            };
        }
    }
    fn bot(&mut self, index: usize) -> &mut ClearBot {
        if self.bots.len() <= index {
            self.bots.resize(index + 1, ClearBot::default());
        }
        &mut self.bots[index]
    }
    /// TerrorNavArea::IsCleared (102e34d0).
    fn is_cleared(&self, area: usize, attributes: u32) -> bool {
        if attributes & (PLAYER_START | CHECKPOINT) != 0 {
            return true;
        }
        attributes & NOT_CLEARABLE == 0 && self.cleared.get(area).copied().unwrap_or(false)
    }
    /// Bot `index` is at `position`; on entering a new area it queues the
    /// uncleared areas that area reveals.
    pub fn enter(&mut self, index: usize, areas: &impl Areas, position: Vec3) {
        let Some(area) = areas.area_at(position) else {
            return;
        };
        if self.bot(index).area == Some(area) {
            return;
        }
        let revealed: Vec<usize> = areas
            .visible_areas(area, COMPLETELY_VISIBLE)
            .into_iter()
            .filter(|v| {
                areas.info(*v).is_some_and(|i| {
                    !self.is_cleared(*v, i.attributes)
                        && i.attributes & (NOT_CLEARABLE | CHECKPOINT | OBSCURED) == 0
                })
            })
            .collect();
        let bot = self.bot(index);
        bot.area = Some(area);
        for v in revealed {
            if !bot.queue.contains(&v) {
                bot.queue.push(v);
            }
        }
    }
    /// A survivor at `position` clears every area completely visible there.
    pub fn mark(&mut self, areas: &impl Areas, position: Vec3) {
        let Some(area) = areas.area_at(position) else {
            return;
        };
        for v in areas.visible_areas(area, COMPLETELY_VISIBLE) {
            if let Some(c) = self.cleared.get_mut(v) {
                *c = true;
            }
        }
    }
    /// The local survivor at `position`: on entering a new area, sums the
    /// extent of the completely visible areas this survivor had not cleared
    /// (AAS update 10335740 -> 1030b720; IsCleared 102e34d0 treats start
    /// and checkpoint areas as cleared and NOT_CLEARABLE ones as never),
    /// then marks them cleared for both the survivor and the team.
    pub fn mark_player(&mut self, areas: &impl Areas, position: Vec3, clear: bool) {
        let Some(area) = areas.area_at(position) else {
            return;
        };
        if self.player_area == Some(area) {
            return;
        }
        self.player_area = Some(area);
        let mut exposed = 0.;
        for v in areas.visible_areas(area, COMPLETELY_VISIBLE) {
            let Some(info) = areas.info(v) else { continue };
            let cleared = info.attributes & (PLAYER_START | CHECKPOINT) != 0
                || (info.attributes & NOT_CLEARABLE == 0 && self.player_cleared.get(v).copied().unwrap_or(false));
            if !cleared {
                exposed += info.size;
            }
            if !clear {
                continue;
            }
            if info.attributes & (PLAYER_START | CHECKPOINT | NOT_CLEARABLE) == 0 {
                if let Some(c) = self.player_cleared.get_mut(v) {
                    *c = true;
                }
            }
            if let Some(c) = self.cleared.get_mut(v) {
                *c = true;
            }
        }
        self.revealed = exposed;
    }
    /// The area the player's last area change exposed (0 once taken).
    pub fn take_revealed(&mut self) -> f32 {
        std::mem::take(&mut self.revealed)
    }
    /// Where a calm bot looks to clear an area, or None with nothing queued.
    pub fn target(
        &mut self,
        index: usize,
        areas: &impl Areas,
        now: f32,
        dt: f32,
        eye: Vec3,
        aim: Vec3,
    ) -> Option<Vec3> {
        let bot = self.bot(index);
        bot.steady = if aim.dot(bot.last_aim) > 0.9998 { bot.steady + dt } else { 0. };
        bot.last_aim = aim;
        if bot.queue.is_empty() {
            return None;
        }
        if now >= bot.prune_at {
            bot.prune_at = now + CLEAR_PRUNE;
            if let Some(current) = bot.area {
                bot.queue.retain(|a| areas.visible(current, *a, POTENTIALLY_VISIBLE));
            }
        }
        let point = |a: usize| areas.info(a).map(|i| (i.center + Vec3::Y * CLEAR_LOOK_HEIGHT, i.size));
        let target = bot.queue.last().and_then(|a| point(*a)).map(|(p, _)| p);
        let steady = bot.steady;
        bot.queue.retain(|a| {
            let Some((p, size)) = point(*a) else {
                return false;
            };
            let to = p - eye;
            if to.length() < CLEAR_REACHED {
                return false;
            }
            !(aim.dot(to.normalize()) > CLEAR_FACING && steady > (size.sqrt() * 0.005).clamp(0., 1.))
        });
        target
    }
    pub fn forget(&mut self, index: usize) {
        let bot = self.bot(index);
        bot.queue.clear();
        bot.area = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        infos: Vec<AreaInfo>,
        vis: Vec<Vec<(usize, u8)>>,
    }
    impl Areas for Fake {
        fn area_at(&self, p: Vec3) -> Option<usize> {
            let a = (p.x / 10.).floor() as usize;
            (a < self.infos.len()).then_some(a)
        }
        fn count(&self) -> usize {
            self.infos.len()
        }
        fn info(&self, a: usize) -> Option<AreaInfo> {
            self.infos.get(a).copied()
        }
        fn visible_areas(&self, from: usize, mask: u8) -> Vec<usize> {
            self.vis[from].iter().filter(|(_, m)| m & mask != 0).map(|(a, _)| *a).collect()
        }
        fn visible(&self, from: usize, to: usize, mask: u8) -> bool {
            from == to || self.visible_areas(from, mask).contains(&to)
        }
        fn battlestations(&self, _: Vec3, _: Option<f32>) -> Vec<usize> {
            vec![]
        }
    }
    #[test]
    fn bots_clear_newly_revealed_areas_then_go_back_to_looking_around() {
        let area = |x: f32, attributes: u32| AreaInfo {
            center: Vec3::new(x, 0., 0.),
            size: 200. * 200.,
            attributes,
        };
        // 0: the start area; 1: a room it reveals; 2: a checkpoint.
        let fake = Fake {
            infos: vec![area(5., PLAYER_START), area(15., 0), area(25., CHECKPOINT)],
            vis: vec![vec![(1, 2), (2, 2)], vec![(0, 3)], vec![(1, 3)]],
        };
        let mut c = Clearing::default();
        c.sync(1, 3);
        let at = Vec3::new(5., 0., 0.);
        let eye = at + Vec3::Y * 1.6;
        c.enter(0, &fake, at);
        assert_eq!(c.bots[0].queue, vec![1], "checkpoints are never queued");
        let target = c.target(0, &fake, 0., 1. / 30., eye, Vec3::X).unwrap();
        assert!((target - Vec3::new(15., CLEAR_LOOK_HEIGHT, 0.)).length() < 1e-4);
        // Facing it with a steady head clears it after sqrt(size) * 0.005 = 1 s.
        let aim = (target - eye).normalize();
        let mut cleared_at = None;
        for tick in 1..60 {
            if c.target(0, &fake, tick as f32 / 30., 1. / 30., eye, aim).is_none() {
                cleared_at = Some(tick);
                break;
            }
        }
        let cleared_at = cleared_at.expect("cleared");
        assert!((30..=33).contains(&cleared_at), "{cleared_at}");
        // Areas a survivor already sees in full are not queued again.
        c.mark(&fake, at);
        c.enter(1, &fake, at);
        assert!(c.bots[1].queue.is_empty());
    }
    #[test]
    fn standing_bots_look_around_and_moving_bots_look_ahead() {
        let mut look = BotLook::default();
        let eye = Vec3::Y * 1.6;
        let friends = [(PLAYER, Vec3::new(0., 1.6, -3.))];
        // Standing: the first pick happens immediately; it never snaps back
        // to the player only, and holds for 3..5 s.
        let first = look.wanted(0, 0., Vec3::ZERO, eye, Vec3::X, &friends, |_, _| true);
        let first = look.wanted(0, 0.1, Vec3::ZERO, eye, first, &friends, |_, _| true);
        assert_eq!(look.wanted(0, 2.9, Vec3::ZERO, eye, first, &friends, |_, _| true), first);
        let mut picks = vec![];
        for t in 0..60 {
            picks.push(look.wanted(0, 6. + t as f32 * 5.1, Vec3::ZERO, eye, Vec3::X, &friends, |_, _| true));
        }
        let at_player = picks.iter().filter(|d| d.dot((friends[0].1 - eye).normalize()) > 0.999).count();
        assert!(at_player > 5 && at_player < 40, "about a third at a friend: {at_player}/60");
        // Random spots sit 27-63 degrees to one side of the view.
        for d in picks.iter().filter(|d| d.dot((friends[0].1 - eye).normalize()) <= 0.999) {
            let yaw = d.with_y(0.).normalize().angle_between(Vec3::X).to_degrees();
            assert!((20. ..=70.).contains(&yaw), "spot yaw {yaw}");
        }
        // Moving: look along the path.
        let mut look = BotLook::default();
        look.wanted(1, 0., Vec3::ZERO, eye, Vec3::X, &friends, |_, _| true);
        let ahead = look.wanted(1, 0.033, Vec3::new(0., 0., 0.2), eye, Vec3::X, &friends, |_, _| true);
        assert!(ahead.dot(Vec3::Z) > 0.99);
        // Head turns at the saccade speed.
        let turned = turn(Vec3::X, Vec3::Z, SACCADE_SPEED, 0.1);
        assert!((turned.angle_between(Vec3::X).to_degrees() - 35.).abs() < 0.5);
    }
}
