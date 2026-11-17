//! Skate mod (ADD-ONS & MODS > MODS): an optional skateboard for the
//! survivor player, enabled from the menu (runtime/mods.ini). Original
//! board physics for this game, everything confined to this module:
//! B board on/off; W kick-pushes (rhythmic kicks up to a cruising speed,
//! hills go faster); S powerslide brake; the board carves toward the view
//! and carving bleeds speed; gravity acts along slopes (rolls back uphill).
//! Hold Space to crouch, release to pop (longer crouch = higher ollie).
//! Airborne: Q kickflip, Z heelflip, X pop shove-it. A finished flip lands
//! clean, a nearly finished one lands sketchy, otherwise it is a bail (as is a
//! huge drop or slamming a wall). Hold C to manual (balance with W/S); land
//! on a ledge or rail edge to grind (balance with A/D, Space pops off).
//! Air tricks, manuals and grinds chain into one combo, banked when the
//! line ends cleanly and lost on a bail or lost balance.
use crate::{
    collision::{Collision, HEIGHT, UNIT},
    game::Body,
    game_state::Match,
};
use bevy::{camera::visibility::RenderLayers, prelude::*};

const SETTINGS: &str = "runtime/mods.ini";
/// Kick push: each kick adds speed up to the cruising cap (units/s);
/// only gravity takes the board faster.
const KICK: f32 = 95. * UNIT;
const KICK_INTERVAL: f32 = 0.55;
const CRUISE: f32 = 430. * UNIT;
const TOP_SPEED: f32 = 900. * UNIT;
/// Rolling resistance, powerslide braking (units/s^2).
const ROLL_FRICTION: f32 = 14. * UNIT;
const BRAKE: f32 = 520. * UNIT;
/// Fraction of speed a full-rate carve bleeds per second.
const CARVE_DRAG: f32 = 0.18;
const CARVE_RATE: f32 = 3.0;
/// Ollie: crouch up to OLLIE_CHARGE seconds for the full pop.
const OLLIE_CHARGE: f32 = 0.35;
const OLLIE_MIN: f32 = crate::player_movement::JUMP * 0.8;
const OLLIE_MAX: f32 = crate::player_movement::JUMP * 1.25;
const OLLIE_SPEED_BONUS: f32 = 0.08;
/// Landing harder than this (vertical units/s) bails.
const MAX_LANDING: f32 = 780. * UNIT;
/// Speed lost into a wall in one tick that slams the rider off.
const SLAM: f32 = 280. * UNIT;
/// A flip this far round lands sketchy instead of bailing.
const SKETCHY: f32 = 0.78;
/// Grind friction and the balance drift rate.
const GRIND_FRICTION: f32 = 40. * UNIT;
const BALANCE_DRIFT: f32 = 0.9;
const BALANCE_CORRECT: f32 = 2.6;
/// Points per second balancing.
const MANUAL_POINTS: f32 = 60.;
const GRIND_POINTS: f32 = 90.;
const REMOUNT: f32 = 1.2;
/// How long a pop waits for the wheels to touch down.
const OLLIE_GRACE: f32 = 0.2;

/// Acceleration along `forward` from gravity on a surface with normal `n`
/// (units/s^2; positive downhill).
pub fn slope_accel(forward: Vec3, n: Vec3) -> f32 {
    if n.y < 0.3 {
        return 0.;
    }
    let along = (forward - n * forward.dot(n)).normalize_or_zero();
    -crate::player_movement::GRAVITY * along.y
}
/// Pop speed for a crouch held `charge` seconds while rolling at `speed`.
pub fn pop_speed(charge: f32, speed: f32) -> f32 {
    let t = (charge / OLLIE_CHARGE).clamp(0., 1.);
    OLLIE_MIN + (OLLIE_MAX - OLLIE_MIN) * t + speed.abs() * OLLIE_SPEED_BONUS
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Landing {
    Clean,
    Sketchy,
    Bail(&'static str),
}
/// Touch-down outcome from the trick in progress and the impact speed.
pub fn landing(trick: Option<(Kind, f32)>, impact: f32) -> Landing {
    if impact > MAX_LANDING {
        return Landing::Bail("Too big a drop");
    }
    match trick {
        Some((kind, time)) if time / kind.duration() >= SKETCHY => Landing::Sketchy,
        Some(_) => Landing::Bail("Flip not finished"),
        None => Landing::Clean,
    }
}

const BOARD_LIFT: f32 = 0.06;

/// Enabled mods, saved to runtime/mods.ini.
#[derive(Resource)]
pub struct Mods {
    pub skate: bool,
}
impl Default for Mods {
    fn default() -> Self {
        let text = std::fs::read_to_string(SETTINGS).unwrap_or_default();
        Self {
            skate: text.lines().any(|l| l.trim() == "skate=1"),
        }
    }
}
impl Mods {
    pub fn save(&self) {
        let _ = std::fs::create_dir_all("runtime");
        if let Err(e) = std::fs::write(SETTINGS, format!("skate={}\n", u8::from(self.skate))) {
            eprintln!("Could not save mods: {e}");
        }
    }
}

/// Whether the survivor is on the board (third-person view, no viewmodel).
static RIDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn riding() -> bool {
    RIDING.load(std::sync::atomic::Ordering::Relaxed)
}
/// Menu toggle request (ADD-ONS & MODS), applied by `latch`.
pub static TOGGLE_SKATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Whether the skate mod is enabled (menu status line).
pub fn skate_enabled(mods: &Mods) -> bool {
    mods.skate
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Kickflip,
    Heelflip,
    ShoveIt,
}
impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Kickflip => "Kickflip",
            Kind::Heelflip => "Heelflip",
            Kind::ShoveIt => "Pop Shove-it",
        }
    }
    fn duration(self) -> f32 {
        match self {
            Kind::Kickflip | Kind::Heelflip => 0.45,
            Kind::ShoveIt => 0.35,
        }
    }
    fn points(self) -> u32 {
        match self {
            Kind::Kickflip | Kind::Heelflip => 100,
            Kind::ShoveIt => 75,
        }
    }
    /// Board rotation at `progress` 0..1 in the board frame (-Z nose, +X right).
    fn rotation(self, progress: f32) -> Quat {
        let turn = std::f32::consts::TAU * progress.clamp(0., 1.);
        match self {
            Kind::Kickflip => Quat::from_rotation_z(-turn),
            Kind::Heelflip => Quat::from_rotation_z(turn),
            Kind::ShoveIt => Quat::from_rotation_y(turn * 0.5),
        }
    }
}

/// Balancing on two wheels or on an edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Balance {
    Manual,
    /// Grinding along `dir` at support height `height`.
    Grind { dir: Vec3, height: f32 },
}
#[derive(Resource, Default)]
pub struct Skate {
    pub riding: bool,
    pub heading: f32,
    /// Signed speed along the heading (negative = rolling backward).
    pub speed: f32,
    pub vertical: f32,
    pub trick: Option<(Kind, f32)>,
    /// Current line: element names and points so far.
    pub line: Vec<String>,
    pub line_points: f32,
    pub score: u32,
    pub balance: Option<(Balance, f32, f32)>,
    pub message: String,
    pub status: String,
    message_time: f32,
    airborne: bool,
    kick_timer: f32,
    charge: f32,
    remount: f32,
    clock: f32,
    // Presses latched in Update for the fixed tick.
    toggle: bool,
    ollie: bool,
    /// Seconds an ollie press has waited for the wheels to touch down
    /// (rolling over small steps leaves the ground for a tick or two).
    ollie_wait: f32,
    /// In the air from a pop (flip tricks need one; rolling off a curb is not).
    popped: bool,
    queued: Option<Kind>,
}
impl Skate {
    fn say(&mut self, text: String) {
        println!("Skate: {text}");
        self.message = text;
        self.message_time = 2.5;
    }
    fn add(&mut self, name: &str, points: f32) {
        if self.line.last().is_none_or(|l| l != name) {
            self.line.push(name.into());
        }
        self.line_points += points;
    }
    /// Ends the line cleanly: points x number of elements.
    fn bank(&mut self) {
        if self.line.is_empty() {
            return;
        }
        let multiplier = self.line.len() as u32;
        let points = (self.line_points.round() as u32) * multiplier;
        self.score += points;
        let names = self.line.join(" + ");
        self.say(if multiplier > 1 {
            format!("{names}  x{multiplier}  +{points}   SCORE {}", self.score)
        } else {
            format!("{names}  +{points}   SCORE {}", self.score)
        });
        self.line.clear();
        self.line_points = 0.;
    }
    fn lose_line(&mut self, why: &str) {
        self.line.clear();
        self.line_points = 0.;
        self.balance = None;
        self.say(why.into());
    }
    fn bail(&mut self, why: &str) {
        self.trick = None;
        self.riding = false;
        self.speed = 0.;
        self.vertical = 0.;
        self.remount = REMOUNT;
        self.lose_line(&format!("BAILED! {why}"));
    }
    /// Advances an airborne trick; a finished one joins the line.
    fn advance_trick(&mut self, dt: f32) {
        if let Some((kind, time)) = &mut self.trick {
            *time += dt;
            let kind = *kind;
            if *time >= kind.duration() {
                self.trick = None;
                self.add(kind.name(), kind.points() as f32);
            }
        }
    }
    /// Touch-down. Returns false on a bail.
    fn land(&mut self, impact: f32, into_manual: bool) -> bool {
        match landing(self.trick, impact) {
            Landing::Bail(why) => {
                self.bail(why);
                false
            }
            Landing::Sketchy => {
                let kind = self.trick.take().unwrap().0;
                self.add(&format!("Sketchy {}", kind.name()), kind.points() as f32 * 0.5);
                self.speed *= 0.6;
                if !into_manual {
                    self.bank();
                }
                true
            }
            Landing::Clean => {
                if !into_manual {
                    self.bank();
                }
                true
            }
        }
    }
    /// Board rotation of the trick in progress.
    pub fn trick_rotation(&self) -> Quat {
        self.trick.map_or(Quat::IDENTITY, |(k, t)| k.rotation(t / k.duration()))
    }
}
/// Height of walkable support under `at` near `level`, if any.
fn support(c: &Collision, at: Vec3, level: f32) -> Option<f32> {
    support_within(c, at, level, 0.45)
}
/// Like `support`, searching `depth` below `level`. A small box (not a ray)
/// so seams in road surfaces are not mistaken for drops.
fn support_within(c: &Collision, at: Vec3, level: f32, depth: f32) -> Option<f32> {
    const PROBE: f32 = 0.04;
    let from = Vec3::new(at.x, level + 0.3, at.z);
    let to = Vec3::new(at.x, level - depth, at.z);
    let hit = c.trace(from, to, Vec3::splat(PROBE));
    (!hit.start_solid && hit.fraction < 1. && hit.normal.y > 0.7).then(|| from.y + (to.y - from.y) * hit.fraction - PROBE)
}
/// A ledge or rail under the board: support here and ahead along the
/// heading, but a drop to at least one side. Returns the grind direction.
fn grindable(c: &Collision, feet: Vec3, forward: Vec3) -> Option<(Vec3, f32)> {
    let height = support(c, feet, feet.y)?;
    if (height - feet.y).abs() > 0.15 {
        return None;
    }
    let ahead = support(c, feet + forward * 0.4, height)?;
    if (ahead - height).abs() > 0.12 {
        return None;
    }
    let right = Vec3::new(-forward.z, 0., forward.x);
    let drops = |side: Vec3| support_within(c, feet + side * 0.3, height, 3.).is_none_or(|h| height - h > 0.25);
    (drops(right) || drops(-right)).then_some((forward, height))
}

pub fn install(app: &mut App) {
    app.init_resource::<Mods>()
        .init_resource::<Skate>()
        .add_systems(Update, (latch, draw_board, hud))
        .add_systems(FixedUpdate, ride.before(crate::game::move_player));
}

fn usable(mods: &Mods, s: &Match) -> bool {
    mods.skate && !s.infected && !s.dead && !s.incap && !s.pinned && !s.paused
}

/// Key presses latched every frame (a fixed tick may not see just_pressed).
fn latch(keys: Res<ButtonInput<KeyCode>>, mut mods: ResMut<Mods>, s: Res<Match>, mut skate: ResMut<Skate>) {
    if TOGGLE_SKATE.swap(false, std::sync::atomic::Ordering::Relaxed) {
        mods.skate = !mods.skate;
        mods.save();
    }
    if !usable(&mods, &s) {
        return;
    }
    skate.toggle |= keys.just_pressed(KeyCode::KeyB);
    if skate.riding {
        skate.ollie |= keys.just_released(KeyCode::Space);
        for (key, kind) in [(KeyCode::KeyQ, Kind::Kickflip), (KeyCode::KeyZ, Kind::Heelflip), (KeyCode::KeyX, Kind::ShoveIt)] {
            if keys.just_pressed(key) {
                skate.queued = Some(kind);
            }
        }
    }
}

/// Held keys for one tick (real keys, or the `--capture-skate` script).
#[derive(Default, Clone, Copy)]
struct Held {
    push: bool,
    brake: bool,
    crouch: bool,
    manual: bool,
    left: bool,
    right: bool,
}

#[allow(clippy::too_many_arguments)]
fn ride(
    time: Res<Time<Fixed>>,
    keys: Res<ButtonInput<KeyCode>>,
    mods: Res<Mods>,
    collision: Res<Collision>,
    mut s: ResMut<Match>,
    mut skate: ResMut<Skate>,
    mut body: Single<&mut Body>,
) {
    let dt = time.delta_secs();
    let mut held = Held {
        push: keys.pressed(KeyCode::KeyW),
        brake: keys.pressed(KeyCode::KeyS),
        crouch: keys.pressed(KeyCode::Space),
        manual: keys.pressed(KeyCode::KeyC),
        left: keys.pressed(KeyCode::KeyA),
        right: keys.pressed(KeyCode::KeyD),
    };
    // `--capture-skate`: scripted line for captures (mod on for this run):
    // kick-push, crouch and pop, kickflip, land into a manual, roll out.
    static CAPTURE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let capture = *CAPTURE.get_or_init(|| std::env::args().any(|a| a == "--capture-skate"));
    let mut capture_mods = Mods { skate: mods.skate };
    if capture {
        capture_mods.skate = true;
        let t = s.elapsed;
        if !skate.riding && (1.0..1.05).contains(&t) {
            skate.toggle = true;
            body.pitch = -0.6;
            // Face the most open of eight directions.
            let c = collision.for_actor(false, false);
            let start = body.feet + Vec3::Y * 0.3;
            body.yaw = (0..8)
                .map(|i| i as f32 * std::f32::consts::FRAC_PI_4)
                .max_by(|a, b| {
                    let open = |yaw: f32| c.trace(start, start + Quat::from_rotation_y(yaw) * Vec3::NEG_Z * 20., Vec3::new(0.2, 0.25, 0.2)).fraction;
                    open(*a).total_cmp(&open(*b))
                })
                .unwrap_or(body.yaw);
        }
        held.push = (1.0..1.7).contains(&t);
        held.crouch = (1.7..2.05).contains(&t);
        if (2.05..2.1).contains(&t) {
            skate.ollie = true;
        }
        if (2.15..2.2).contains(&t) && skate.trick.is_none() && skate.line.is_empty() {
            skate.queued = Some(Kind::Kickflip);
        }
        held.manual = (2.5..3.6).contains(&t);
        // Keep the manual balanced like a player would.
        if let Some((Balance::Manual, b, _)) = skate.balance {
            held.push = b > 0.15;
            held.brake = b < -0.15;
        }
    }
    let mods = &capture_mods;
    skate.message_time = (skate.message_time - dt).max(0.);
    if skate.message_time <= 0. {
        skate.message.clear();
    }
    skate.remount = (skate.remount - dt).max(0.);
    skate.clock += dt;
    RIDING.store(skate.riding && usable(mods, &s), std::sync::atomic::Ordering::Relaxed);
    if !usable(mods, &s) {
        if skate.riding {
            skate.riding = false;
            skate.trick = None;
            skate.balance = None;
            skate.line.clear();
            skate.line_points = 0.;
        }
        skate.toggle = false;
        skate.ollie = false;
        skate.queued = None;
        skate.status.clear();
        return;
    }
    let c = collision.for_actor(false, false);
    let grounded = body.velocity.y <= 140. * UNIT && c.grounded(body.feet, HEIGHT);
    if std::mem::take(&mut skate.toggle) {
        if skate.riding {
            skate.riding = false;
            skate.trick = None;
            skate.balance = None;
            skate.bank();
        } else if grounded && skate.remount <= 0. {
            skate.riding = true;
            skate.heading = body.yaw;
            let forward = Quat::from_rotation_y(body.yaw) * Vec3::NEG_Z;
            skate.speed = (body.velocity * Vec3::new(1., 0., 1.)).dot(forward);
            skate.vertical = 0.;
            skate.airborne = false;
            skate.kick_timer = 0.;
            skate.say("Skateboard: W kick, S brake, hold+release Space ollie, Q/Z/X flips, C manual, land on edges to grind".into());
        }
    }
    if !skate.riding {
        skate.ollie = false;
        skate.queued = None;
        skate.status.clear();
        RIDING.store(false, std::sync::atomic::Ordering::Relaxed);
        return;
    }
    let forward = Quat::from_rotation_y(skate.heading) * Vec3::NEG_Z;
    // ---- Grinding: locked to the edge until it ends, a pop or a bad balance.
    if let Some((Balance::Grind { dir, height }, balance, lean)) = skate.balance {
        skate.add("50-50 Grind", GRIND_POINTS * dt);
        let lean = lean + (skate.clock * 1.7).sin() * 0.6 * dt;
        let correct = (held.right as i32 - held.left as i32) as f32 * BALANCE_CORRECT;
        let balance = balance + (lean * BALANCE_DRIFT + correct) * dt;
        skate.speed = (skate.speed - GRIND_FRICTION * dt).max(0.);
        let step = dir * skate.speed * dt;
        let target = body.feet + step;
        let ahead = support(&c, target, height);
        let blocked = c.trace(body.feet + Vec3::Y * 0.3, target + Vec3::Y * 0.3, Vec3::splat(0.2)).fraction < 1.;
        if balance.abs() >= 1. {
            skate.balance = None;
            skate.bail("Lost balance on the grind");
            body.velocity = Vec3::ZERO;
            return finish(&mut skate, &mut body, grounded);
        }
        if std::mem::take(&mut skate.ollie) || skate.speed < 40. * UNIT {
            skate.balance = None;
            skate.popped = true;
            skate.vertical = pop_speed(skate.charge.max(0.1), skate.speed);
            skate.charge = 0.;
        } else if blocked {
            skate.balance = None;
            skate.speed = 0.;
            skate.bank();
        } else if let Some(h) = ahead.filter(|h| (h - height).abs() < 0.15) {
            skate.balance = Some((Balance::Grind { dir, height: h }, balance, lean));
            body.feet = Vec3::new(target.x, h, target.z);
            body.velocity = dir * skate.speed;
            body.grounded = true;
            skate.status = format!("50-50 GRIND  {}  {:.0} pts", meter(balance), skate.line_points);
            return finish(&mut skate, &mut body, true);
        } else {
            // Off the end of the ledge or rail.
            skate.balance = None;
            skate.vertical = 0.;
        }
    }
    // ---- Rolling.
    let mut manual = matches!(skate.balance, Some((Balance::Manual, ..)));
    if grounded && !skate.airborne {
        let carve = CARVE_RATE / (1. + skate.speed.abs() / CRUISE);
        let delta = (body.yaw - skate.heading + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        let turn = delta.clamp(-carve * dt, carve * dt);
        skate.heading += turn;
        skate.speed *= 1. - CARVE_DRAG * (turn.abs() / dt / CARVE_RATE).min(1.) * dt;
        // Gravity along the slope.
        let ground = c.trace(body.feet + Vec3::Y * 0.1, body.feet - Vec3::Y * 0.4, Vec3::ZERO);
        if ground.fraction < 1. {
            skate.speed += slope_accel(forward, ground.normal) * dt;
        }
        if manual {
            // W/S balance the manual instead of pushing/braking.
            let (_, balance, lean) = skate.balance.unwrap();
            let lean = lean + (skate.clock * 2.3).sin() * 0.5 * dt;
            let correct = (held.brake as i32 - held.push as i32) as f32 * BALANCE_CORRECT;
            let balance = balance + (lean * BALANCE_DRIFT + correct) * dt;
            skate.add("Manual", MANUAL_POINTS * dt);
            if balance.abs() >= 1. {
                skate.lose_line("Manual broken: line lost");
                manual = false;
            } else if !held.manual {
                skate.balance = None;
                skate.bank();
                manual = false;
            } else {
                skate.balance = Some((Balance::Manual, balance, lean));
                skate.status = format!("MANUAL  {}  {:.0} pts", meter(balance), skate.line_points);
            }
        } else {
            skate.kick_timer = (skate.kick_timer - dt).max(0.);
            if held.push && skate.kick_timer <= 0. && skate.speed < CRUISE {
                skate.speed = (skate.speed.max(0.) + KICK).min(CRUISE);
                skate.kick_timer = KICK_INTERVAL;
            }
            if held.brake {
                let brake = BRAKE * dt;
                skate.speed = if skate.speed > 0. { (skate.speed - brake).max(0.) } else { (skate.speed + brake).min(0.) };
            }
            if held.manual && skate.speed.abs() > 60. * UNIT {
                skate.balance = Some((Balance::Manual, 0., if skate.clock.sin() > 0. { 0.4 } else { -0.4 }));
            }
        }
        let friction = ROLL_FRICTION * dt;
        skate.speed = if skate.speed > 0. { (skate.speed - friction).max(0.) } else { (skate.speed + friction).min(0.) };
        skate.speed = skate.speed.clamp(-TOP_SPEED * 0.5, TOP_SPEED);
        // Crouch then pop.
        if held.crouch {
            skate.charge = (skate.charge + dt).min(OLLIE_CHARGE);
        }
        if std::mem::take(&mut skate.ollie) {
            skate.ollie_wait = 0.;
            skate.popped = true;
            skate.vertical = pop_speed(skate.charge, skate.speed);
            skate.charge = 0.;
            if manual {
                skate.balance = None;
            }
        } else {
            skate.vertical = 0.;
            // Flip presses while rolling are not saved for later.
            skate.queued = None;
        }
    } else {
        if skate.ollie {
            skate.ollie_wait += dt;
            if skate.ollie_wait > OLLIE_GRACE {
                skate.ollie = false;
            }
        }
        if !skate.ollie {
            skate.ollie_wait = 0.;
            skate.charge = 0.;
        }
        skate.vertical -= crate::player_movement::GRAVITY * dt;
        if skate.trick.is_none() && skate.popped {
            if let Some(kind) = skate.queued.take() {
                skate.trick = Some((kind, 0.));
            }
        }
    }
    if !manual && !matches!(skate.balance, Some((Balance::Grind { .. }, ..))) {
        skate.status.clear();
    }
    skate.advance_trick(dt);
    let forward = Quat::from_rotation_y(skate.heading) * Vec3::NEG_Z;
    let velocity = forward * skate.speed + Vec3::Y * skate.vertical;
    let on_ground = grounded && skate.vertical <= 0.;
    let (mut next, contact) = c.walk_velocity(body.feet, velocity, dt, HEIGHT, on_ground);
    if on_ground {
        next = c.snap_ground(next, HEIGHT);
    }
    // Walls take the speed they absorb; a hard hit slams the rider off.
    let kept = (contact * Vec3::new(1., 0., 1.)).dot(forward);
    if skate.speed.abs() - kept.abs() > SLAM {
        skate.bail("Slammed into a wall");
        body.velocity = Vec3::ZERO;
        return finish(&mut skate, &mut body, grounded);
    }
    skate.speed = if skate.speed >= 0. { kept.max(0.).min(skate.speed) } else { kept.min(0.).max(skate.speed) };
    let falling = skate.vertical;
    if skate.airborne && falling <= 0. {
        // Landing on a ledge or rail edge locks into a grind.
        if let Some((dir, height)) = grindable(&c, next, forward).filter(|_| skate.trick.is_none() && skate.speed > 60. * UNIT) {
            skate.balance = Some((Balance::Grind { dir, height }, 0., if skate.clock.cos() > 0. { 0.5 } else { -0.5 }));
            body.feet = Vec3::new(next.x, height, next.z);
            body.velocity = dir * skate.speed;
            skate.vertical = 0.;
            skate.airborne = false;
            return finish(&mut skate, &mut body, true);
        }
    }
    let mut landed = c.grounded(next, HEIGHT) && contact.y <= 0. && falling <= 0.;
    // Wheels stay planted over small dips and slope changes unless popped.
    if !landed && on_ground && !skate.popped {
        if let Some(h) = support_within(&c, next, next.y, 0.3) {
            next.y = h;
            landed = true;
        }
    }
    if skate.airborne && landed {
        if !skate.land(-falling, held.manual) {
            body.velocity = Vec3::ZERO;
            body.feet = next;
            s.stagger = 0.8;
            s.stagger_dir = forward;
            return finish(&mut skate, &mut body, true);
        }
        if held.manual && skate.speed.abs() > 60. * UNIT {
            skate.balance = Some((Balance::Manual, 0., if skate.clock.sin() > 0. { 0.4 } else { -0.4 }));
        }
    }
    skate.vertical = if landed { 0. } else { contact.y };
    skate.airborne = !landed;
    if landed {
        skate.popped = false;
    }
    body.velocity = Vec3::new(contact.x, skate.vertical, contact.z);
    body.feet = next;
    finish(&mut skate, &mut body, landed)
}
fn finish(skate: &mut Skate, body: &mut Body, grounded: bool) {
    body.grounded = grounded;
    RIDING.store(skate.riding, std::sync::atomic::Ordering::Relaxed);
}
/// Balance meter: -1 (falls left/back) .. 1.
fn meter(balance: f32) -> String {
    let slots = 11;
    let at = (((balance.clamp(-1., 1.) + 1.) * 0.5) * (slots - 1) as f32).round() as usize;
    (0..slots).map(|i| if i == at { '|' } else if i == slots / 2 { '+' } else { '-' }).collect::<String>()
}

#[derive(Component)]
struct Board;

/// The board under the survivor's feet (procedural deck, trucks and wheels).
fn draw_board(
    mut commands: Commands,
    skate: Res<Skate>,
    body: Single<&Body>,
    fixed: Res<Time<Fixed>>,
    mut boards: Query<(&mut Transform, &mut Visibility), With<Board>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Ok((mut transform, mut visibility)) = boards.single_mut() else {
        // Flat colours: the map's exposure washes default-lit materials out.
        let flat = |r: f32, g: f32, b: f32| StandardMaterial { base_color: Color::srgb(r, g, b), unlit: true, ..default() };
        let deck = materials.add(flat(0.32, 0.18, 0.09));
        let grip = materials.add(flat(0.04, 0.04, 0.04));
        let metal = materials.add(flat(0.45, 0.46, 0.48));
        let wheel = materials.add(flat(0.75, 0.72, 0.62));
        let root = commands
            .spawn((Board, Transform::default(), Visibility::Hidden, RenderLayers::layer(1)))
            .id();
        let parts = [
            (Cuboid::new(0.2, 0.014, 0.78), deck, Vec3::new(0., 0.075, 0.)),
            (Cuboid::new(0.19, 0.004, 0.74), grip, Vec3::new(0., 0.084, 0.)),
            (Cuboid::new(0.16, 0.02, 0.04), metal.clone(), Vec3::new(0., 0.058, -0.25)),
            (Cuboid::new(0.16, 0.02, 0.04), metal, Vec3::new(0., 0.058, 0.25)),
        ];
        for (shape, material, at) in parts {
            commands.spawn((Mesh3d(meshes.add(shape)), MeshMaterial3d(material), Transform::from_translation(at), RenderLayers::layer(1), ChildOf(root)));
        }
        for (x, z) in [(-0.085, -0.25), (0.085, -0.25), (-0.085, 0.25), (0.085, 0.25)] {
            commands.spawn((
                Mesh3d(meshes.add(Cylinder::new(0.028, 0.03))),
                MeshMaterial3d(wheel.clone()),
                Transform::from_xyz(x, 0.028, z).with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)),
                RenderLayers::layer(1),
                ChildOf(root),
            ));
        }
        return;
    };
    let wanted = if skate.riding { Visibility::Inherited } else { Visibility::Hidden };
    if *visibility != wanted {
        *visibility = wanted;
    }
    if !skate.riding {
        return;
    }
    let feet = body.previous_feet.lerp(body.feet, fixed.overstep_fraction());
    // Flip about the board's centre: lift it, rotate, put it back.
    let centre = Vec3::Y * 0.08;
    let heading = Quat::from_rotation_y(skate.heading);
    // A manual rides tail-down on the back wheels.
    let trick = if matches!(skate.balance, Some((Balance::Manual, ..))) {
        Quat::from_rotation_x(0.22)
    } else {
        skate.trick_rotation()
    };
    let lift = if skate.trick.is_some() { 0.15 } else { BOARD_LIFT - 0.06 };
    *transform = Transform::from_translation(feet + Vec3::Y * lift + heading * (centre - trick * centre))
        .with_rotation(heading * trick);
}

#[derive(Component)]
struct SkateText;

/// Trick and score line under the crosshair.
fn hud(
    mut commands: Commands,
    skate: Res<Skate>,
    frontend: Res<crate::game_menu::Frontend>,
    camera: Option<Single<Entity, With<crate::game::ViewCamera>>>,
    mut texts: Query<(&mut Text, &mut Node), With<SkateText>>,
) {
    let Ok((mut text, mut node)) = texts.single_mut() else {
        if let Some(camera) = camera {
            commands.spawn((
                Text::new(""),
                TextFont { font_size: bevy::text::FontSize::Vh(2.6), ..default() },
                TextColor(Color::srgb(1., 0.85, 0.3)),
                TextLayout::new(Justify::Center, LineBreak::WordBoundary),
                Node { position_type: PositionType::Absolute, top: percent(60.), width: percent(100.), ..default() },
                UiTargetCamera(*camera),
                SkateText,
            ));
        }
        return;
    };
    let line = match (skate.status.is_empty(), skate.message.is_empty()) {
        (true, _) => skate.message.clone(),
        (false, true) => skate.status.clone(),
        (false, false) => format!("{}
{}", skate.status, skate.message),
    };
    let display = if frontend.playing() && !line.is_empty() { Display::Flex } else { Display::None };
    if node.display != display {
        node.display = display;
    }
    if text.0 != line {
        text.0 = line;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lines_bank_clean_landings_and_bails_lose_them() {
        let mut skate = Skate { riding: true, ..default() };
        skate.trick = Some((Kind::Kickflip, 0.));
        skate.advance_trick(0.5);
        skate.trick = Some((Kind::ShoveIt, 0.));
        skate.advance_trick(0.4);
        assert_eq!(skate.line, vec!["Kickflip", "Pop Shove-it"]);
        assert!(skate.land(200. * UNIT, false));
        // (100 + 75) x2.
        assert_eq!(skate.score, 350);
        assert!(skate.riding && skate.line.is_empty());
        // Nearly round: sketchy, half points, speed lost.
        skate.speed = 10.;
        skate.trick = Some((Kind::Heelflip, Kind::Heelflip.duration() * 0.85));
        assert!(skate.land(200. * UNIT, false));
        assert_eq!(skate.score, 400);
        assert!((skate.speed - 6.).abs() < 1e-4);
        // Barely started: bail, nothing banked, off the board.
        skate.trick = Some((Kind::Heelflip, 0.05));
        assert!(!skate.land(200. * UNIT, false));
        assert!(!skate.riding && skate.score == 400);
        // A clean landing into a manual keeps the line going.
        let mut skate = Skate { riding: true, ..default() };
        skate.add("Kickflip", 100.);
        assert!(skate.land(100. * UNIT, true));
        assert_eq!(skate.line, vec!["Kickflip"]);
        // Too big a drop bails even without a trick.
        assert_eq!(landing(None, 900. * UNIT), Landing::Bail("Too big a drop"));
    }
    #[test]
    fn slopes_pop_and_flip_rotation() {
        // A 20 degree slope whose normal leans toward +Z falls away toward +Z.
        let n = (Vec3::Y + Vec3::Z * 0.364).normalize();
        assert!(slope_accel(Vec3::Z, n) > 0., "downhill speeds up");
        assert!(slope_accel(Vec3::NEG_Z, n) < 0., "uphill slows down");
        assert_eq!(slope_accel(Vec3::NEG_Z, Vec3::Y), 0.);
        // A longer crouch pops higher; rolling adds a little.
        assert!(pop_speed(OLLIE_CHARGE, 0.) > pop_speed(0., 0.));
        assert!(pop_speed(0.1, 5.) > pop_speed(0.1, 0.));
        // A full kickflip turns the board once about its long axis.
        let end = Kind::Kickflip.rotation(1.) * Vec3::Y;
        let half = Kind::Kickflip.rotation(0.5) * Vec3::Y;
        assert!(end.dot(Vec3::Y) > 0.999 && half.dot(Vec3::Y) < -0.999);
        assert_eq!(meter(0.), "-----+-----".replace('+', "|"));
    }
}
