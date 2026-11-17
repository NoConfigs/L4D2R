//! Multiplayer stage 2b: one horde for everyone, run by the host.
//!
//! - The host's game spawns and runs every infected. 15 times a second it
//!   sends a snapshot ("k":"inf") of the infected near any survivor: a
//!   stable network id, position, yaw, health and the `Enemy` fields the
//!   animation reads (speed, reaction, attack and their serials), plus the
//!   AI special or Witch activity name and, once dead, the death context.
//!   New ids also carry the model, kind and common appearance.
//! - Guests spawn nothing themselves (game.rs `director` stops on guests and
//!   `drop_local` removes any other local spawn). They draw a puppet `Enemy`
//!   per id, 150 ms behind between snapshots, and run no AI on it.
//! - A guest's own damage to a puppet (any path: bullets, melee, shoves,
//!   grenades) is seen as a health drop and sent to the host as "k":"hit";
//!   the host applies it to the real infected. The guest keeps its lower
//!   health for a moment so its own kills fall at once.
//! - The host forwards what its infected do to a guest's survivor (damage,
//!   pins, revives) from `net_play::apply`; `own_player` applies it here.
use crate::game::Enemy;
use crate::multiplayer::Session;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};

/// Snapshots per second.
const RATE: f64 = 15.;
/// Puppets are drawn this far in the past, between two snapshots.
const DELAY: f64 = 0.15;
/// Infected farther than this from every survivor are not sent (metres).
const RANGE: f32 = 70.;
/// Most infected in one snapshot (nearest first).
const MOST: usize = 160;
/// Every this many snapshots, the spawn details of every sent id again.
const RESEND: u64 = 30;

/// What kind of actor an id is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Common,
    Special,
    Witch,
}
impl Kind {
    fn code(self) -> u8 {
        match self {
            Self::Common => 0,
            Self::Special => 1,
            Self::Witch => 2,
        }
    }
    fn from_code(code: u64) -> Self {
        match code {
            1 => Self::Special,
            2 => Self::Witch,
            _ => Self::Common,
        }
    }
}

/// Spawn details of an id (sent when it is new, and now and then again).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Spawn {
    pub id: u32,
    pub model: String,
    pub kind: Kind,
    /// Common appearance (skin, clothing, blood sheet).
    pub look: Option<[u8; 3]>,
}

/// How an infected died (infected_death::Context).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Death {
    pub cause: u8,
    pub head: bool,
    pub running: bool,
    /// Incoming direction in the infected's own frame.
    pub incoming: Vec3,
}

/// One infected in a snapshot.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Infected {
    pub id: u32,
    pub position: Vec3,
    pub yaw: f32,
    pub health: f32,
    pub speed: f32,
    pub reaction: u8,
    pub attack: f32,
    pub attack_serial: u64,
    pub reaction_serial: u64,
    pub vertical: f32,
    pub burning: bool,
    /// AI special / Witch activity, and the special's gesture.
    pub activity: Option<String>,
    pub gesture: Option<String>,
    pub death: Option<Death>,
    /// A Smoker's tongue (mouth, bends, tip or victim), when out.
    pub tongue: Option<Vec<Vec3>>,
}

fn r2(v: f32) -> f64 {
    (v as f64 * 100.).round() / 100.
}
fn r3(v: f32) -> f64 {
    (v as f64 * 1000.).round() / 1000.
}

fn cause_code(c: crate::infected_death::Cause) -> u8 {
    use crate::infected_death::Cause::*;
    match c {
        Bullet => 0,
        Shotgun => 1,
        Slash => 2,
        Blunt => 3,
        Fire => 4,
        Explosion => 5,
    }
}
fn cause_from(code: u8) -> crate::infected_death::Cause {
    use crate::infected_death::Cause::*;
    match code {
        1 => Shotgun,
        2 => Slash,
        3 => Blunt,
        4 => Fire,
        5 => Explosion,
        _ => Bullet,
    }
}
impl Death {
    fn of(c: &crate::infected_death::Context) -> Self {
        Self { cause: cause_code(c.cause), head: c.head, running: c.running, incoming: c.incoming }
    }
    fn context(&self) -> crate::infected_death::Context {
        crate::infected_death::Context {
            cause: cause_from(self.cause),
            head: self.head,
            running: self.running,
            incoming: self.incoming,
            ragdoll: false,
            far: false,
        }
    }
}

impl Spawn {
    fn to_json(&self) -> Value {
        let mut v = json!([self.id, self.model, self.kind.code()]);
        if let Some([a, b, c]) = self.look {
            v.as_array_mut().unwrap().extend([json!(a), json!(b), json!(c)]);
        }
        v
    }
    fn from_json(v: &Value) -> Option<Self> {
        let a = v.as_array()?;
        let look = (a.len() >= 6).then(|| {
            let n = |i: usize| a[i].as_u64().unwrap_or(0).min(255) as u8;
            [n(3), n(4), n(5)]
        });
        Some(Self {
            id: a.first()?.as_u64()? as u32,
            model: a.get(1)?.as_str()?.to_string(),
            kind: Kind::from_code(a.get(2)?.as_u64()?),
            look,
        })
    }
}

impl Infected {
    /// A compact array: id, x, y, z, yaw (degrees), health, speed, reaction,
    /// attack, attack serial, reaction serial, vertical, burning, activity,
    /// gesture, death [cause, head, running, ix, iy, iz].
    fn to_json(&self) -> Value {
        let p = self.position;
        json!([
            self.id,
            r2(p.x),
            r2(p.y),
            r2(p.z),
            self.yaw.to_degrees().round() as i32,
            r2(self.health),
            r2(self.speed),
            self.reaction,
            r2(self.attack),
            self.attack_serial,
            self.reaction_serial,
            r2(self.vertical),
            self.burning as u8,
            self.activity,
            self.gesture,
            self.death.map(|d| json!([d.cause, d.head as u8, d.running as u8, r3(d.incoming.x), r3(d.incoming.y), r3(d.incoming.z)])),
            self.tongue.as_ref().map(|path| path.iter().flat_map(|p| [r2(p.x), r2(p.y), r2(p.z)]).collect::<Vec<_>>()),
        ])
    }
    fn from_json(v: &Value) -> Option<Self> {
        let a = v.as_array()?;
        let f = |i: usize| a.get(i).and_then(Value::as_f64).unwrap_or(0.) as f32;
        let u = |i: usize| a.get(i).and_then(Value::as_u64).unwrap_or(0);
        let s = |i: usize| a.get(i).and_then(Value::as_str).map(str::to_string);
        let position = Vec3::new(a.get(1)?.as_f64()? as f32, f(2), f(3));
        if !position.is_finite() {
            return None;
        }
        let death = a.get(15).and_then(Value::as_array).map(|d| {
            let g = |i: usize| d.get(i).and_then(Value::as_f64).unwrap_or(0.) as f32;
            Death {
                cause: g(0) as u8,
                head: g(1) != 0.,
                running: g(2) != 0.,
                incoming: Vec3::new(g(3), g(4), g(5)),
            }
        });
        Some(Self {
            id: a.first()?.as_u64()? as u32,
            position,
            yaw: f(4).to_radians(),
            health: f(5),
            speed: f(6),
            reaction: u(7).min(255) as u8,
            attack: f(8),
            attack_serial: u(9),
            reaction_serial: u(10),
            vertical: f(11),
            burning: u(12) != 0,
            activity: s(13),
            gesture: s(14),
            death,
            tongue: a.get(16).and_then(Value::as_array).map(|t| {
                t.chunks(3)
                    .filter_map(|c| Some(Vec3::new(c.first()?.as_f64()? as f32, c.get(1)?.as_f64()? as f32, c.get(2)?.as_f64()? as f32)))
                    .filter(|p| p.is_finite())
                    .collect()
            }),
        })
    }
}

/// A snapshot message.
pub(crate) fn snapshot_json(serial: u64, spawns: &[Spawn], infected: &[Infected]) -> Value {
    json!({
        "k": "inf",
        "n": serial,
        "s": spawns.iter().map(Spawn::to_json).collect::<Vec<_>>(),
        "e": infected.iter().map(Infected::to_json).collect::<Vec<_>>(),
    })
}
pub(crate) fn parse_snapshot(m: &Value) -> Option<(Vec<Spawn>, Vec<Infected>)> {
    if m.get("k")?.as_str()? != "inf" {
        return None;
    }
    let spawns = m.get("s")?.as_array()?.iter().filter_map(Spawn::from_json).collect();
    let infected = m.get("e")?.as_array()?.iter().filter_map(Infected::from_json).collect();
    Some((spawns, infected))
}

/// A guest's hit on a puppet, for the host.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Hit {
    pub id: u32,
    pub damage: f32,
    /// 0 none, 1 shoved, 2 flinched.
    pub reaction: u8,
    /// The guest's killing blow (death cause etc.).
    pub death: Option<Death>,
    /// World direction of the hit (horizontal).
    pub direction: Vec3,
}
impl Hit {
    pub(crate) fn to_json(&self) -> Value {
        let mut v = json!({
            "k": "hit",
            "i": self.id,
            "d": r2(self.damage),
            "r": self.reaction,
            "dir": [r3(self.direction.x), r3(self.direction.z)],
        });
        if let Some(d) = self.death {
            v["c"] = json!(d.cause);
            v["hd"] = json!(d.head);
        }
        v
    }
    pub(crate) fn from_json(m: &Value) -> Option<Self> {
        if m.get("k")?.as_str()? != "hit" {
            return None;
        }
        let damage = m.get("d").and_then(Value::as_f64).unwrap_or(0.) as f32;
        if !damage.is_finite() || !(0. ..=10_000.).contains(&damage) {
            return None;
        }
        let dir = m.get("dir").and_then(Value::as_array);
        let g = |i: usize| dir.and_then(|d| d.get(i)).and_then(Value::as_f64).unwrap_or(0.) as f32;
        Some(Self {
            id: m.get("i")?.as_u64()? as u32,
            damage,
            reaction: m.get("r").and_then(Value::as_u64).unwrap_or(0).min(2) as u8,
            death: m.get("c").and_then(Value::as_u64).map(|c| Death {
                cause: c as u8,
                head: m.get("hd").and_then(Value::as_bool).unwrap_or(false),
                running: false,
                incoming: Vec3::ZERO,
            }),
            direction: Vec3::new(g(0), 0., g(1)).normalize_or_zero(),
        })
    }
}

/// Host: stable network ids for infected entities.
#[derive(Default)]
pub(crate) struct Ids {
    by_entity: HashMap<Entity, u32>,
    by_id: HashMap<u32, Entity>,
    next: u32,
}
impl Ids {
    pub(crate) fn id(&mut self, entity: Entity) -> (u32, bool) {
        if let Some(id) = self.by_entity.get(&entity) {
            return (*id, false);
        }
        self.next = self.next.wrapping_add(1).max(1);
        let id = self.next;
        self.by_entity.insert(entity, id);
        self.by_id.insert(id, entity);
        (id, true)
    }
    pub(crate) fn entity(&self, id: u32) -> Option<Entity> {
        self.by_id.get(&id).copied()
    }
    /// Forget entities that no longer exist.
    pub(crate) fn retain(&mut self, alive: &HashSet<Entity>) {
        self.by_entity.retain(|e, _| alive.contains(e));
        self.by_id.retain(|_, e| alive.contains(e));
    }
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Shortest-way blend between two yaws (radians).
pub(crate) fn blend_yaw(a: f32, b: f32, t: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let d = (b - a).rem_euclid(tau);
    let d = if d > std::f32::consts::PI { d - tau } else { d };
    a + d * t
}

/// Received poses of a puppet, oldest first.
#[derive(Default, Debug)]
pub(crate) struct Path(VecDeque<(f64, Vec3, f32)>);
impl Path {
    pub(crate) fn push(&mut self, at: f64, position: Vec3, yaw: f32) {
        self.0.push_back((at, position, yaw));
        while self.0.len() > 2 && self.0[1].0 < at - 1. {
            self.0.pop_front();
        }
    }
    /// The pose at `at` (receive-time clock).
    pub(crate) fn sample(&self, at: f64) -> Option<(Vec3, f32)> {
        let (t0, p0, y0) = *self.0.front()?;
        if at <= t0 {
            return Some((p0, y0));
        }
        for i in 1..self.0.len() {
            let (a, pa, ya) = self.0[i - 1];
            let (b, pb, yb) = self.0[i];
            if at <= b {
                let t = if b > a { ((at - a) / (b - a)) as f32 } else { 1. };
                // A jump (teleport, ladder) is not slid across.
                if pa.distance(pb) > 6. {
                    return Some((pb, yb));
                }
                return Some((pa.lerp(pb, t), blend_yaw(ya, yb, t)));
            }
        }
        self.0.back().map(|(_, p, y)| (*p, *y))
    }
}

/// Guest: an infected drawn for the host's.
#[derive(Component)]
pub(crate) struct Puppet {
    pub id: u32,
    path: Path,
    /// The health last taken from the host (or our own lower one).
    expected: f32,
    reaction_serial: u64,
    /// Our own hits stand over the host's health until then.
    predict_until: f64,
    dead: bool,
    seen: f64,
}

/// Activity names as the `&'static str` the animation components hold.
fn intern(name: &str) -> &'static str {
    static NAMES: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(n) = names.iter().find(|n| **n == name) {
        return n;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    names.push(leaked);
    leaked
}

fn debug() -> bool {
    std::env::var_os("L4D2R_NET_DEBUG").is_some()
}

#[derive(Resource, Default)]
pub(crate) struct NetInfected {
    ids: Ids,
    sent: HashSet<u32>,
    serial: u64,
    last_send: f64,
    /// Guest: spawn details by id; puppets by id.
    spawns: HashMap<u32, Spawn>,
    puppets: HashMap<u32, Entity>,
    missing_models: HashSet<String>,
    hits_sent: u64,
    hits_applied: u64,
    last_log: f64,
}
impl NetInfected {
    /// Host: the network id of an infected already sent to guests.
    pub(crate) fn net_id(&self, entity: Entity) -> Option<u32> {
        self.ids.by_entity.get(&entity).copied()
    }
    /// Guest: the puppet standing in for the host's infected `id`.
    pub(crate) fn puppet(&self, id: u32) -> Option<Entity> {
        self.puppets.get(&id).copied()
    }
}

/// Gore hit codes for the network: the infected_death cause.
pub(crate) fn gore_cause_code(c: crate::infected_death::Cause) -> u8 {
    cause_code(c)
}
pub(crate) fn gore_cause(code: u8) -> crate::infected_death::Cause {
    cause_from(code)
}

/// Update (host): sends snapshots and applies the guests' hits.
#[allow(clippy::too_many_arguments)]
fn host(
    mut net: ResMut<NetInfected>,
    mut play: ResMut<crate::net_play::NetPlay>,
    mut session: ResMut<Session>,
    time: Res<Time<Real>>,
    body: Option<Single<&crate::game::Body>>,
    squad: Option<Res<crate::game_state::Squad>>,
    mut enemies: Query<(
        Entity,
        &mut Transform,
        &mut Enemy,
        Option<&crate::special_infected::ai::AiActivity>,
        Option<&crate::special_infected::witch::WitchActivity>,
        Has<crate::special_infected::ai::AiSpecial>,
        Has<crate::special_infected::witch::Witch>,
        Option<&crate::infected_appearance::Appearance>,
    )>,
    animation: Option<Res<crate::l4d2r_animation::AnimationLibrary>>,
    collision: Option<Res<crate::collision::Collision>>,
    smokers: Query<&crate::special_infected::ai::AiSpecial>,
) {
    if !crate::net_play::host() {
        if !crate::net_play::guest() {
            play.other.clear();
        }
        if !net.ids.by_entity.is_empty() || !net.sent.is_empty() {
            net.ids.clear();
            net.sent.clear();
        }
        return;
    }
    let net = &mut *net;
    // Guests' hits.
    for (from, message) in std::mem::take(&mut play.other) {
        let Some(hit) = Hit::from_json(&message) else { continue };
        let Some(entity) = net.ids.entity(hit.id) else { continue };
        let Ok((entity, mut transform, mut enemy, _, _, special, witch, _)) = enemies.get_mut(entity) else { continue };
        if enemy.health <= 0. {
            continue;
        }
        net.hits_applied += 1;
        if let Some(index) = squad.as_deref().and_then(|q| play.member_index(q, from)) {
            crate::game_ui::kill_feed::hit(entity, crate::game_ui::kill_feed::Killer::Bot(index));
        }
        let kind = crate::infected_population::Kind::model(&enemy.model);
        enemy.health -= hit.damage;
        let seed = |e: &Enemy| entity.to_bits().wrapping_add(e.reaction_serial.wrapping_mul(7919));
        if hit.reaction == 1 {
            // As game.rs `combat` shoves: Riot turn, stagger and push.
            if kind == crate::infected_population::Kind::Riot {
                transform.rotation *= Quat::from_rotation_y(std::f32::consts::PI);
            }
            enemy.reaction_serial = enemy.reaction_serial.wrapping_add(1);
            enemy.attack = animation.as_ref().map_or(0.7, |a| {
                a.infected_reaction_duration(&enemy.model, "ACT_TERROR_SHOVED_BACKWARD", seed(&enemy))
            });
            enemy.reaction = 1;
            if let Some(c) = collision.as_deref() {
                let h = crate::collision::HEIGHT;
                transform.translation =
                    c.walk(transform.translation, hit.direction * 0.55, h, c.grounded(transform.translation, h));
            }
        } else if hit.reaction == 2 {
            enemy.reaction_serial = enemy.reaction_serial.wrapping_add(1);
            // Commons flinch as a gesture (see game::combat).
            if !enemy.model.contains("/common_") {
                enemy.attack = animation.as_ref().map_or(0.5, |a| {
                    a.infected_reaction_duration(&enemy.model, "ACT_TERROR_FLINCH", seed(&enemy))
                });
                enemy.reaction = 2;
            }
        }
        // Specials and the Witch handle their own deaths; commons die as in
        // `combat`.
        if enemy.health <= 0. && !special && !witch {
            let death = hit.death.unwrap_or(Death { cause: 0, head: false, running: false, incoming: Vec3::ZERO });
            let t = *transform;
            enemy.record_death(cause_from(death.cause), &t, hit.direction, if death.head { 1.6 } else { 1. });
            enemy.death.head |= death.head;
            enemy.corpse_time = animation.as_ref().map_or(5., |a| a.death_duration(&enemy.model, enemy.death, seed(&enemy)) + 6.);
        }
    }
    let now = time.elapsed_secs_f64();
    if now - net.last_send < 1. / RATE {
        return;
    }
    net.last_send = now;
    let mut survivors = vec![];
    if let Some(body) = body.as_deref() {
        survivors.push(body.feet);
    }
    if let Some(squad) = squad.as_deref() {
        survivors.extend(squad.0.iter().filter(|m| !m.state.dead).map(|m| m.position));
    }
    let alive: HashSet<Entity> = enemies.iter().map(|e| e.0).collect();
    net.ids.retain(&alive);
    let mut near: Vec<(f32, Entity)> = enemies
        .iter()
        .filter_map(|(entity, t, ..)| {
            let d = survivors.iter().map(|p| p.distance(t.translation)).fold(f32::INFINITY, f32::min);
            (d < RANGE).then_some((d, entity))
        })
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    near.truncate(MOST);
    net.serial += 1;
    let resend = net.serial % RESEND == 0;
    let mut spawns = vec![];
    let mut infected = vec![];
    let mut sent = HashSet::new();
    for (_, entity) in near {
        let Ok((_, t, e, ai, witch_act, special, witch, look)) = enemies.get(entity) else { continue };
        let (id, _) = net.ids.id(entity);
        sent.insert(id);
        if resend || !net.sent.contains(&id) {
            spawns.push(Spawn {
                id,
                model: e.model.clone(),
                kind: if special {
                    Kind::Special
                } else if witch {
                    Kind::Witch
                } else {
                    Kind::Common
                },
                look: look.map(|a| [a.skin, a.cloth, a.sheet]),
            });
        }
        infected.push(Infected {
            id,
            position: t.translation,
            yaw: t.rotation.to_euler(EulerRot::YXZ).0,
            health: e.health,
            speed: e.movement_speed,
            reaction: e.reaction,
            attack: e.attack,
            attack_serial: e.attack_serial,
            reaction_serial: e.reaction_serial,
            vertical: e.vertical,
            burning: e.burning > 0.,
            activity: ai.map(|a| a.0.to_string()).or(witch_act.map(|w| w.0.to_string())),
            gesture: ai.and_then(|a| a.1).map(str::to_string),
            death: (e.health <= 0.).then(|| Death::of(&e.death)),
            tongue: smokers.get(entity).ok().zip(squad.as_deref()).and_then(|(ai, squad)| {
                let player = body.as_ref().map_or(Vec3::ZERO, |b| b.feet);
                crate::special_infected::ai::tongue_path(t, ai, player, squad)
            }),
        });
    }
    net.sent = sent;
    if debug() && now - net.last_log > 2. {
        net.last_log = now;
        let ids: Vec<u32> = infected.iter().take(6).map(|i| i.id).collect();
        println!(
            "Net infected (host): sending {} infected (ids {ids:?}...), {} live in the world, {} guest hits applied",
            infected.len(),
            alive.len(),
            net.hits_applied
        );
    }
    session.core.game(snapshot_json(net.serial, &spawns, &infected));
}

/// Update (guest): forwards our hits on puppets, then applies snapshots.
#[allow(clippy::too_many_arguments)]
fn guest(
    mut commands: Commands,
    mut net: ResMut<NetInfected>,
    mut play: ResMut<crate::net_play::NetPlay>,
    time: Res<Time<Real>>,
    models: Option<Res<crate::viewer::RenderModels>>,
    animation: Option<Res<crate::l4d2r_animation::AnimationLibrary>>,
    body: Option<Single<&crate::game::Body>>,
    mut puppets: Query<(Entity, &mut Transform, &mut Enemy, &mut Puppet)>,
    mut ai: Query<&mut crate::special_infected::ai::AiActivity>,
    mut witch: Query<&mut crate::special_infected::witch::WitchActivity>,
) {
    let now = time.elapsed_secs_f64();
    if !crate::net_play::guest() {
        if !net.puppets.is_empty() {
            for (_, entity) in net.puppets.drain() {
                commands.entity(entity).try_despawn();
            }
        }
        net.spawns.clear();
        return;
    }
    let net = &mut *net;
    let feet = body.as_deref().map_or(Vec3::ZERO, |b| b.feet);
    // Our own damage to puppets goes to the host.
    for (_, transform, enemy, mut puppet) in &mut puppets {
        if puppet.dead && puppet.expected <= 0. {
            continue;
        }
        let damage = puppet.expected - enemy.health;
        let reacted = enemy.reaction_serial != puppet.reaction_serial;
        if damage > 0.01 || reacted {
            let killed = enemy.health <= 0.;
            let hit = Hit {
                id: puppet.id,
                damage: damage.max(0.),
                reaction: if reacted && matches!(enemy.reaction, 1 | 2) { enemy.reaction } else { 0 },
                death: killed.then(|| Death::of(&enemy.death)),
                direction: (transform.translation - feet) * Vec3::new(1., 0., 1.),
            };
            play.outgoing.push(hit.to_json());
            net.hits_sent += 1;
            puppet.expected = enemy.health;
            puppet.reaction_serial = enemy.reaction_serial;
            puppet.predict_until = now + 0.5;
            if killed {
                puppet.dead = true;
            }
        }
    }
    // Snapshots: the latest of this frame.
    let mut latest = None;
    for (_, message) in std::mem::take(&mut play.other) {
        if message.get("k").and_then(Value::as_str) == Some("inf") {
            latest = Some(message);
        } else {
            // Messages for our own player (own_player reads them next).
            play.other.push((0, message));
        }
    }
    if let Some(message) = latest {
        if let Some((spawns, infected)) = parse_snapshot(&message) {
            for s in spawns {
                net.spawns.insert(s.id, s);
            }
            let present: HashSet<u32> = infected.iter().map(|i| i.id).collect();
            for state in infected {
                if let Some(entity) = net.puppets.get(&state.id).copied() {
                    if let Ok((entity, _, mut enemy, mut puppet)) = puppets.get_mut(entity) {
                        update_puppet(entity, &mut enemy, &mut puppet, &state, now, animation.as_deref());
                        // The Smoker's tongue, drawn by special_infected::ai::draw_tongues.
                        match &state.tongue {
                            Some(path) => {
                                commands.entity(entity).insert(crate::special_infected::ai::NetTongue(path.clone()));
                            }
                            None => {
                                commands.entity(entity).remove::<crate::special_infected::ai::NetTongue>();
                            }
                        }
                        if let Some(name) = state.activity.as_deref() {
                            if let Ok(mut a) = ai.get_mut(entity) {
                                a.0 = intern(name);
                                a.1 = state.gesture.as_deref().map(intern);
                            } else if let Ok(mut w) = witch.get_mut(entity) {
                                w.0 = intern(name);
                            }
                        }
                        continue;
                    }
                    net.puppets.remove(&state.id);
                }
                if state.health <= 0. {
                    continue;
                }
                let Some(spawn) = net.spawns.get(&state.id) else { continue };
                let Some(models) = models.as_deref() else { continue };
                if let Some(entity) = spawn_puppet(&mut commands, models, spawn, &state, now) {
                    net.puppets.insert(state.id, entity);
                } else if net.missing_models.insert(spawn.model.clone()) {
                    println!("Net infected: {} is not loaded here; not drawn", spawn.model);
                }
            }
            // Gone from the snapshot: living ones go at once; corpses lie
            // until their own timer ends.
            net.puppets.retain(|id, entity| {
                if present.contains(id) {
                    return true;
                }
                match puppets.get(*entity) {
                    Ok((_, _, enemy, _)) if enemy.health <= 0. => true,
                    Ok(_) => {
                        commands.entity(*entity).try_despawn();
                        false
                    }
                    Err(_) => false,
                }
            });
            net.spawns.retain(|id, _| present.contains(id));
        }
    }
    // Move puppets along their paths; run corpse timers.
    let dt = time.delta_secs();
    let at = now - DELAY;
    for (entity, mut transform, mut enemy, puppet) in &mut puppets {
        if enemy.health <= 0. {
            enemy.corpse_time -= dt;
            if enemy.corpse_time <= 0. || now - puppet.seen > 60. {
                commands.entity(entity).try_despawn();
                net.puppets.retain(|_, e| *e != entity);
            }
            continue;
        }
        if let Some((position, yaw)) = puppet.path.sample(at) {
            transform.translation = position;
            transform.rotation = Quat::from_rotation_y(yaw);
        }
    }
    if debug() && now - net.last_log > 2. {
        net.last_log = now;
        let mut ids: Vec<u32> = net.puppets.keys().copied().collect();
        ids.sort_unstable();
        println!(
            "Net infected (guest): {} puppets (ids {:?}...), {} hits sent",
            net.puppets.len(),
            &ids[..ids.len().min(6)],
            net.hits_sent
        );
    }
}

fn update_puppet(
    entity: Entity,
    enemy: &mut Enemy,
    puppet: &mut Puppet,
    state: &Infected,
    now: f64,
    animation: Option<&crate::l4d2r_animation::AnimationLibrary>,
) {
    puppet.seen = now;
    if puppet.dead && enemy.health <= 0. {
        return;
    }
    puppet.path.push(now, state.position, state.yaw);
    // Our own recent hits stand until the host has had them.
    let health = if now < puppet.predict_until { state.health.min(enemy.health) } else { state.health };
    enemy.movement_speed = state.speed;
    enemy.reaction = state.reaction;
    enemy.attack = state.attack;
    enemy.attack_serial = state.attack_serial;
    if state.reaction_serial > enemy.reaction_serial || now >= puppet.predict_until {
        enemy.reaction_serial = state.reaction_serial;
    }
    enemy.vertical = state.vertical;
    enemy.burning = if state.burning { enemy.burning.max(0.5) } else { 0. };
    puppet.reaction_serial = enemy.reaction_serial;
    if health <= 0. && enemy.health > 0. || state.death.is_some() && !puppet.dead {
        // The host's kill: the same death here.
        if let Some(d) = state.death {
            enemy.death = d.context();
        }
        let seed = entity.to_bits().wrapping_add(enemy.reaction_serial.wrapping_mul(7919));
        enemy.corpse_time = if crate::special_infected::SpecialClass::from_model(&enemy.model).is_some()
            || enemy.model.contains("witch")
        {
            12.
        } else {
            animation.map_or(5., |a| a.death_duration(&enemy.model, enemy.death, seed) + 6.)
        };
        puppet.dead = true;
    }
    enemy.health = health;
    puppet.expected = health;
}

fn spawn_puppet(
    commands: &mut Commands,
    models: &crate::viewer::RenderModels,
    spawn: &Spawn,
    state: &Infected,
    now: f64,
) -> Option<Entity> {
    let draws = models.0.get(&(spawn.model.clone(), 0, 0))?;
    let mut path = Path::default();
    path.push(now, state.position, state.yaw);
    let mut actor = commands.spawn((
        Transform::from_translation(state.position).with_rotation(Quat::from_rotation_y(state.yaw)),
        Visibility::default(),
        Enemy {
            model: spawn.model.clone(),
            death_loot: None,
            loot_dropped: false,
            reaction_serial: state.reaction_serial,
            attack_serial: state.attack_serial,
            health: state.health,
            attack: state.attack,
            vertical: state.vertical,
            reaction: state.reaction,
            corpse_time: 0.,
            death: Default::default(),
            movement_speed: state.speed,
            burning: 0.,
        },
        Puppet {
            id: state.id,
            path,
            expected: state.health,
            reaction_serial: state.reaction_serial,
            predict_until: 0.,
            dead: false,
            seen: now,
        },
    ));
    let activity = intern(state.activity.as_deref().unwrap_or("ACT_IDLE"));
    match spawn.kind {
        Kind::Special => {
            actor.insert(crate::special_infected::ai::AiActivity(activity, state.gesture.as_deref().map(intern)));
        }
        Kind::Witch => {
            actor.insert(crate::special_infected::witch::WitchActivity(activity));
        }
        Kind::Common => {
            if let Some([skin, cloth, sheet]) = spawn.look {
                actor.insert(crate::infected_appearance::Appearance { skin, cloth, sheet, burn: 0 });
            }
        }
    }
    let actor = actor.id();
    let smoker = spawn.model == crate::special_infected::SpecialClass::Smoker.model();
    for (index, (mesh, material)) in draws.iter().enumerate() {
        // smoker.mdl draw 1 is the extended tongue (special_infected::ai::spawn).
        if smoker && index == 1 {
            continue;
        }
        commands.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            crate::viewer::DynamicModelLighting::default(),
            Transform::default(),
            RenderLayers::layer(1),
            ChildOf(actor),
        ));
    }
    Some(actor)
}

/// Update (guest): the host's damage, pins and revives of our survivor.
fn own_player(
    mut play: ResMut<crate::net_play::NetPlay>,
    state: Option<ResMut<crate::game_state::Match>>,
    body: Option<Single<&mut crate::game::Body>>,
    settings: Option<Res<crate::campaign_settings::SessionSettings>>,
) {
    if !crate::net_play::guest() {
        return;
    }
    let (Some(mut state), Some(mut body), Some(settings)) = (state, body, settings) else {
        play.other.clear();
        return;
    };
    let me = settings.survivor.name();
    for (_, m) in std::mem::take(&mut play.other) {
        if m.get("s").and_then(Value::as_str) != Some(me) {
            continue;
        }
        match m.get("k").and_then(Value::as_str) {
            Some("dmg") => {
                let amount = m.get("a").and_then(Value::as_f64).unwrap_or(0.) as f32;
                state.damage(amount);
                if debug() {
                    println!("Net infected: the host's infected hit us for {amount}");
                }
            }
            Some("vom") => {
                let seconds = m.get("t").and_then(Value::as_f64).unwrap_or(0.) as f32;
                if seconds.is_finite() && seconds > 0. {
                    state.vomited = state.vomited.max(seconds.min(30.));
                    if debug() {
                        println!("Net infected: vomited on in the host's game ({seconds} s)");
                    }
                }
            }
            Some("rvp") => {
                let seconds = m.get("t").and_then(Value::as_f64).unwrap_or(0.) as f32;
                if seconds.is_finite() && state.incap && !state.dead {
                    crate::net_play::set_revive_progress(seconds.clamp(0., 5.));
                }
            }
            Some("rev") => {
                if state.incap && !state.dead {
                    state.revive();
                    println!("Net infected: revived in the host's game");
                }
            }
            Some("pin") => {
                let by = m.get("by").and_then(Value::as_str).and_then(crate::special_infected::SpecialClass::parse);
                if by.is_some() != state.pinned && debug() {
                    println!("Net infected: pinned by {by:?} in the host's game (None: released)");
                }
                state.pinned = by.is_some();
                state.pinned_by = by;
                state.pin_allows_fire = by.is_some() && m.get("f").and_then(Value::as_bool).unwrap_or(false);
                if let Some(p) = m.get("p").and_then(Value::as_array) {
                    let g = |i: usize| p.get(i).and_then(Value::as_f64).map(|v| v as f32);
                    if let (Some(x), Some(y), Some(z)) = (g(0), g(1), g(2)) {
                        let to = Vec3::new(x, y, z);
                        if to.is_finite() {
                            body.feet = to;
                            body.previous_feet = to;
                            body.velocity = Vec3::ZERO;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Guests: an infected our own game spawned anyway (console, map logic) is
/// removed; the host's game owns the horde.
fn drop_local(mut commands: Commands, fresh: Query<Entity, (Added<Enemy>, Without<Puppet>)>) {
    if !crate::net_play::guest() {
        return;
    }
    for entity in &fresh {
        commands.entity(entity).try_despawn();
    }
}

pub fn install(app: &mut App) {
    app.init_resource::<NetInfected>().add_systems(
        Update,
        (host, guest, own_player, drop_local)
            .chain()
            .after(crate::net_play::exchange)
            .before(crate::l4d2r_animation::update),
    );
    app.add_systems(FixedUpdate, drop_local);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn infected(id: u32) -> Infected {
        Infected {
            id,
            position: Vec3::new(1.25, -2.5, 30.),
            yaw: 90f32.to_radians(),
            health: 37.5,
            speed: 4.2,
            reaction: 2,
            attack: 0.5,
            attack_serial: 7,
            reaction_serial: 9,
            vertical: 0.,
            burning: true,
            activity: Some("ACT_RUN".into()),
            gesture: None,
            death: Some(Death { cause: 3, head: true, running: false, incoming: Vec3::new(0., 0., -1.) }),
            tongue: Some(vec![Vec3::new(1., 2., 3.), Vec3::new(4.5, 5., 6.)]),
        }
    }

    #[test]
    fn snapshots_round_trip_through_json() {
        let spawns = vec![
            Spawn { id: 4, model: "models/infected/common_male01.mdl".into(), kind: Kind::Common, look: Some([3, 9, 1]) },
            Spawn { id: 5, model: "models/infected/hunter.mdl".into(), kind: Kind::Special, look: None },
        ];
        let list = vec![infected(4), Infected { death: None, activity: None, ..infected(5) }];
        let text = snapshot_json(12, &spawns, &list).to_string();
        let (s, e) = parse_snapshot(&serde_json::from_str(&text).unwrap()).unwrap();
        assert_eq!(s, spawns);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].id, 4);
        assert!((e[0].yaw - list[0].yaw).abs() < 1e-3);
        assert_eq!(Infected { yaw: list[0].yaw, ..e[0].clone() }, list[0]);
        assert_eq!(e[1].death, None);
        assert_eq!(e[1].activity, None);
        assert!(parse_snapshot(&json!({"k": "p"})).is_none());
    }

    #[test]
    fn hits_round_trip_and_reject_bad_damage() {
        let hit = Hit {
            id: 77,
            damage: 12.5,
            reaction: 1,
            death: Some(Death { cause: 2, head: true, running: false, incoming: Vec3::ZERO }),
            direction: Vec3::X,
        };
        assert_eq!(Hit::from_json(&hit.to_json()).unwrap(), hit);
        let mut bad = hit.to_json();
        bad["d"] = json!(-5.);
        assert!(Hit::from_json(&bad).is_none());
        assert!(Hit::from_json(&json!({"k": "inf"})).is_none());
    }

    #[test]
    fn ids_are_stable_and_forget_despawned_entities() {
        let mut ids = Ids::default();
        let a = Entity::from_raw_u32(10).unwrap();
        let b = Entity::from_raw_u32(11).unwrap();
        let (ia, new_a) = ids.id(a);
        let (ib, _) = ids.id(b);
        assert!(new_a && ia != ib);
        assert_eq!(ids.id(a), (ia, false));
        assert_eq!(ids.entity(ib), Some(b));
        ids.retain(&HashSet::from([b]));
        assert_eq!(ids.entity(ia), None);
        // A new entity never reuses an old id.
        let (ia2, _) = ids.id(a);
        assert!(ia2 != ia && ia2 != ib);
    }

    #[test]
    fn puppets_are_drawn_between_snapshots() {
        let mut path = Path::default();
        path.push(1.0, Vec3::ZERO, 350f32.to_radians());
        path.push(1.1, Vec3::X * 2., 10f32.to_radians());
        assert_eq!(path.sample(0.5).unwrap().0, Vec3::ZERO);
        let (p, yaw) = path.sample(1.05).unwrap();
        assert!((p.x - 1.).abs() < 1e-4);
        // The short way round through 0 degrees.
        assert!(yaw.to_degrees().rem_euclid(360.) < 0.01 || yaw.to_degrees().rem_euclid(360.) > 359.99);
        assert_eq!(path.sample(5.).unwrap().0, Vec3::X * 2.);
        // A teleport is not slid across.
        path.push(1.2, Vec3::X * 50., 0.);
        assert_eq!(path.sample(1.15).unwrap().0, Vec3::X * 50.);
    }

    #[test]
    fn death_context_survives_the_wire() {
        let c = crate::infected_death::Context {
            cause: crate::infected_death::Cause::Explosion,
            head: false,
            running: true,
            incoming: Vec3::new(0.5, 0., 0.5),
            ragdoll: false,
            far: false,
        };
        let d = Death::of(&c);
        let back = d.context();
        assert_eq!(back.cause, c.cause);
        assert_eq!(back.running, c.running);
        assert_eq!(back.incoming, c.incoming);
    }
}
