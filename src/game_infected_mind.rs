//! Unalerted common infected: the retail NextBot InfectedWander and
//! InfectedAlert behaviours (server.dll terror/zombiebot/infected). A common
//! without a `Mind` (a mob or horde spawn) attacks at once; a wanderer idles
//! in its posture until it notices a survivor, becomes alert, acquires the
//! survivor and only then attacks.
use crate::collision::UNIT;
use bevy::prelude::*;

/// z_vision_range / z_vision_range_alert.
pub(crate) const VISION_RANGE: f32 = 500.;
pub(crate) const VISION_RANGE_ALERT: f32 = 1500.;
/// z_notice_near_range: noticed in any direction this close.
pub(crate) const NOTICE_NEAR_RANGE: f32 = 100.;
/// Half the NextBot default field of view (90 degrees).
const VISION_HALF_FOV_COS: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// z_hear_gunfire_range, z_hear_runner_near_range, z_hear_runner_far_range.
pub(crate) const HEAR_GUNFIRE_RANGE: f32 = 200.;
pub(crate) const HEAR_RUNNER_NEAR_RANGE: f32 = 500.;
pub(crate) const HEAR_RUNNER_FAR_RANGE: f32 = 750.;
/// A survivor must have been running this long to be heard (1041a770).
const RUNNER_TIME: f32 = 2.;
/// The far runner range needs the runner heading at the common (dot < -0.9396).
const RUNNER_FAR_DOT: f32 = -0.9396;
/// z_attention_range_sitting_modifier / z_attention_range_lying_modifier.
const SITTING_ATTENTION: f32 = 0.35;
const LYING_ATTENTION: f32 = 0.15;
/// z_acquire_near_range/far_range, near_time/far_time and
/// time_variance_factor (InfectedAlert::Update 10407350).
const ACQUIRE_NEAR_RANGE: f32 = 200.;
const ACQUIRE_FAR_RANGE: f32 = 2500.;
const ACQUIRE_NEAR_TIME: f32 = 0.5;
const ACQUIRE_FAR_TIME: f32 = 5.;
const ACQUIRE_VARIANCE: f32 = 0.25;
/// z_stumble_max_curve_rate (degrees/s) and z_stumble_max_curve_accel.
const STUMBLE_CURVE_RATE: f32 = 10.;
const STUMBLE_CURVE_ACCEL: f32 = 5.;
/// Shambling walk speed (units/s) when no walk clip is playing; each clip
/// in WALK_CLIPS carries its own.
pub(crate) const SHAMBLE_SPEED: f32 = 22.;
/// z_lean_wall_align_speed (degrees/s): InfectedLeanOnWall turns to its
/// lean yaw at this rate.
const LEAN_ALIGN_SPEED: f32 = 300.;
/// InfectedLeanOnWall::OnStart (server 10418ac0): RandomFloat(5, 20) seconds.
const LEAN_MIN: f32 = 5.;
const LEAN_MAX: f32 = 20.;
/// InfectedLeanOnWall::OnStart: the wall normal against the common's forward
/// (cos 45 degrees) picks forward, backward or a side lean.
const LEAN_DOT: f32 = 0.7071;
/// InfectedStaggerAround::OnContact (10419100): a world surface whose normal
/// z is under 0.1 is a wall to lean on.
pub(crate) const WALL_NORMAL_MAX_UP: f32 = 0.1;
/// The stagger's walk clips (non-looping, each a single shamble with its
/// own extracted root motion, some diagonal or backward): label and length
/// in seconds. A clip that plays to its end hands over to a standing idle.
const WALK_CLIPS: [(&str, f32); 7] = [
    ("Walk_Neutral_01", 12.133),
    ("Walk_Neutral_02", 12.3),
    ("Walk_Neutral_02b", 17.3),
    ("Walk_Neutral_03", 13.333),
    ("Walk_Neutral_04a", 14.433),
    ("Walk_Neutral_04b", 14.167),
    ("Walk_Neutral_South", 7.467),
];
/// ACT_TERROR_IDLE_NEUTRAL clips (non-looping): played once after a walk
/// clip ends, then the common walks again (c1m1_hotel recording).
const IDLE_CLIPS: [(&str, f32); 16] = [
    ("Idle_Neutral_01", 6.3),
    ("Idle_Neutral_02", 21.4),
    ("Idle_Neutral_03", 4.5),
    ("Idle_Neutral_06", 12.833),
    ("Idle_Neutral_07", 8.067),
    ("Idle_Neutral_08", 8.5),
    ("Idle_Neutral_10", 11.633),
    ("Idle_Neutral_11", 8.967),
    ("Idle_Neutral_12", 17.567),
    ("Idle_Neutral_13", 4.9),
    ("Idle_Neutral_14", 16.8),
    ("Idle_Neutral_15", 9.333),
    ("Idle_Neutral_16", 6.667),
    ("Idle_Neutral_17", 7.9),
    ("Idle_Crouching_Neutral_01", 18.8),
    ("Idle_Crouching_Neutral_02", 14.7),
];
/// Retail switches clips just before the end (the next clip's blend).
const CLIP_HANDOVER: f32 = 0.95;
/// Standing turn clips: label, length and the cycle at which retail hands
/// back to the walk (c1m1_hotel recording). The body turns by the clip's
/// own extracted motion (about faces -197, +162 and -161 degrees, left +74,
/// right -104).
pub(crate) struct TurnClip {
    label: &'static str,
    length: f32,
    end: f32,
}
const ABOUT_FACE: [TurnClip; 3] = [
    TurnClip { label: "Idle_Neutral_Turn_Around_01a", length: 3.233, end: 0.80 },
    TurnClip { label: "Idle_Neutral_Turn_Around_01b", length: 4.1, end: 0.854 },
    TurnClip { label: "Idle_Neutral_Turn_Around_01c", length: 3.767, end: 0.865 },
];
const FACE_LEFT: TurnClip = TurnClip { label: "Idle_Neutral_Turn_Left", length: 3.733, end: 0.857 };
/// Recorded on c1m1_hotel (12 November): hands back to the walk at 0.824.
const FACE_RIGHT: TurnClip = TurnClip { label: "Idle_Neutral_Turn_Right", length: 3.033, end: 0.824 };
/// Turn rate while facing a noticed survivor (degrees/s).
const FACE_RATE: f32 = 200.;
/// Get-up clips (Sitting_to_Standing_Alert, Lying_to_Standing_Alert) and
/// Idle_Neutral_to_Alert_01 / alert-to-neutral lengths in seconds.
const SIT_TO_STAND: f32 = 1.667;
const LIE_TO_STAND: f32 = 1.733;
/// Idle_Alert_to_Neutral_01 hands back to the wander at cycle 0.70 of its
/// 1.867 s (recorded on c1m1_hotel, 13 November: always 1.3 s).
const ALERT_TO_NEUTRAL: f32 = 1.3;
/// Common clips' fade-out: a non-looping clip counts as finished this long
/// before its end (recorded startle alerts calmed at cycle 0.42-0.68).
const CLIP_FADE_OUT: f32 = 0.75;
/// InfectedAlert acquire clips (ACT_TERROR_IDLE_ACQUIRE, weight 1 each).
const ACQUIRE_CLIPS: [(&str, f32); 3] = [("Idle_Acquire_05", 6.267), ("Idle_Acquire_06", 6.6), ("Idle_Acquire_11", 7.067)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Posture {
    /// InfectedStaggerAround: ACT_TERROR_WALK_NEUTRAL, slowly curving.
    Stagger,
    /// InfectedSitDown.
    Sit,
    /// InfectedLieDown.
    Lie,
}
impl Posture {
    /// InfectedWander's contained action (104195d0): RandomInt(0, 100)
    /// <= 14 lies down, < 30 sits, otherwise staggers around.
    pub(crate) fn roll(roll: i32) -> Self {
        if roll <= 14 {
            Self::Lie
        } else if roll < 30 {
            Self::Sit
        } else {
            Self::Stagger
        }
    }
    /// Notice delay scale (FUN_10419a60): lying 3, sitting 2, standing 1.
    fn notice_scale(self) -> f32 {
        match self {
            Self::Lie => 3.,
            Self::Sit => 2.,
            Self::Stagger => 1.,
        }
    }
    /// Hearing scale (FUN_10419740).
    fn attention(self) -> f32 {
        match self {
            Self::Lie => LYING_ATTENTION,
            Self::Sit => SITTING_ATTENTION,
            Self::Stagger => 1.,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Stagger {
    /// A walk clip (index into WALK_CLIPS) from `start` until `until`.
    Walk { clip: usize, start: f32, until: f32 },
    /// A standing idle clip (index into IDLE_CLIPS).
    Stand { clip: usize, start: f32, until: f32 },
    /// InfectedStandingActivity with a turn clip.
    Turn { clip: TurnKind, start: f32 },
    /// InfectedLeanOnWall: turning to `goal`, held until `until`.
    Lean { activity: &'static str, goal: f32, until: f32 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TurnKind {
    About(usize),
    Left,
    Right,
}
impl TurnKind {
    fn clip(self) -> &'static TurnClip {
        match self {
            Self::About(i) => &ABOUT_FACE[i],
            Self::Left => &FACE_LEFT,
            Self::Right => &FACE_RIGHT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum State {
    /// Idle in the posture; `notice` is (deadline, survivor position).
    Wander { notice: Option<(f32, Vec3)> },
    /// Getting up from sitting or lying before turning alert.
    Rising { until: f32, toward: Vec3 },
    /// InfectedAlert: looking for or at a survivor. `injured` plays the
    /// ACT_TERROR_IDLE_ALERT_INJURED_* set (alert started by damage).
    Alert { toward: Vec3, calm_at: f32, acquire_at: Option<f32>, injured: bool },
    /// ACT_TERROR_ALERT_TO_NEUTRAL, then back to wandering.
    Calming { until: f32 },
    /// InfectedAttack: the regular chase takes over.
    Attack,
}

/// What a common perceives about one survivor this tick.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Threat {
    pub position: Vec3,
    /// Horizontal facing of the survivor.
    pub forward: Vec3,
    /// Clear line of sight from the common's eyes.
    pub visible: bool,
    /// Seconds the survivor has been running.
    pub running: f32,
    /// The survivor fired a weapon this tick.
    pub fired: bool,
}

pub(crate) struct Senses<'a> {
    pub clock: f32,
    pub position: Vec3,
    pub threats: &'a [Threat],
    /// A mob or finale is underway (InfectedAlert attacks at once).
    pub mob: bool,
    /// Hurt or shoved this tick (OnInjured / OnShoved attack).
    pub injured: bool,
}

/// What the common does this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Intent {
    /// Hold position with this activity, turning toward `yaw`.
    Idle { activity: &'static str, yaw: f32 },
    /// Shamble along `yaw`.
    Walk { yaw: f32 },
    /// Play this clip (started at `start` on the brain's clock); the body
    /// moves and turns by the clip's extracted root motion (Source moves a
    /// wandering common by its animation). The caller adds the turn to
    /// `Mind::yaw`.
    Clip { label: &'static str, start: f32 },
    /// Hand over to the attack chase.
    Attack,
}

#[derive(Component, Clone, Debug)]
pub(crate) struct Mind {
    pub posture: Posture,
    pub state: State,
    pub yaw: f32,
    curve: f32,
    curve_max: f32,
    groan_at: f32,
    seed: u32,
    /// Current activity, read by the animation system.
    pub activity: &'static str,
    /// Health and reaction serial last seen, to notice injuries and shoves.
    pub last: Option<(f32, u64)>,
    /// The staggering wanderer's current clip or action.
    pub stagger: Option<Stagger>,
    /// Where the brain last thought from (alert direction).
    position: Vec3,
    /// InfectedAlert's current clip: (label, length, start).
    alert_clip: Option<(&'static str, f32, f32)>,
}

/// InfectedAlert clips by activity (common models; label, seconds, weight).
/// The violent startles carry weight 2, the long looks 1. Each moves and
/// turns the body by its extracted motion, like the wander clips.
const ALERT_CLIPS: &[(&str, &[(&str, f32, u32)])] = &[
    ("ACT_TERROR_IDLE_ALERT_AHEAD", &[("Idle_Alert_Straight_05", 16.7, 1), ("Idle_Alert_Straight_08", 17.967, 1)]),
    ("ACT_TERROR_IDLE_ALERT_BEHIND", &[
        ("Idle_Alert_Behind_02", 17.433, 1), ("Idle_Alert_Behind_03", 15.833, 1),
        ("violent_alert01_Common_D", 1.967, 2), ("violent_alert01_Common_E", 1.967, 2),
    ]),
    ("ACT_TERROR_IDLE_ALERT_LEFT", &[
        ("Idle_Alert_Left_04", 14.4, 1), ("Idle_Alert_Left_01", 17.633, 1), ("violent_alert01_Common_B", 1.833, 2),
    ]),
    ("ACT_TERROR_IDLE_ALERT_RIGHT", &[
        ("Idle_Alert_Right_02a", 15.9, 1), ("Idle_Alert_Right_02d", 20.8, 1), ("violent_alert01_Common_C", 1.3, 2),
    ]),
    ("ACT_TERROR_IDLE_ALERT_INJURED_AHEAD", &[("Idle_Alert_Injured_Straight_02", 12.5, 1)]),
    ("ACT_TERROR_IDLE_ALERT_INJURED_BEHIND", &[("Idle_Alert_Injured_Behind_01", 12.3, 1)]),
    ("ACT_TERROR_IDLE_ALERT_INJURED_LEFT", &[("Idle_Alert_Injured_Left_02", 10.867, 1)]),
    ("ACT_TERROR_IDLE_ALERT_INJURED_RIGHT", &[("Idle_Alert_Injured_Right_02", 11.2, 1)]),
];

/// The yaw convention of `enemy_ai`: rotation_y(atan2(-z, x)).
pub(crate) fn yaw_of(direction: Vec3) -> f32 {
    (-direction.z).atan2(direction.x)
}
pub(crate) fn forward(yaw: f32) -> Vec3 {
    Vec3::new(yaw.cos(), 0., -yaw.sin())
}
pub(crate) fn turn_toward(from: f32, to: f32, max: f32) -> f32 {
    let mut delta = (to - from) % std::f32::consts::TAU;
    if delta > std::f32::consts::PI {
        delta -= std::f32::consts::TAU;
    } else if delta < -std::f32::consts::PI {
        delta += std::f32::consts::TAU;
    }
    from + delta.clamp(-max, max)
}

impl Mind {
    pub(crate) fn wanderer(seed: u64, posture: Option<Posture>) -> Self {
        let mut mind = Self {
            posture: Posture::Stagger,
            state: State::Wander { notice: None },
            yaw: 0.,
            curve: 0.,
            curve_max: 0.,
            groan_at: 0.,
            seed: (seed as u32 ^ (seed >> 32) as u32).max(1),
            activity: "ACT_TERROR_IDLE_NEUTRAL",
            last: None,
            stagger: None,
            position: Vec3::ZERO,
            alert_clip: None,
        };
        mind.posture = posture.unwrap_or_else(|| Posture::roll(mind.int(0, 100)));
        // Low Violence: InfectedWander's lie-down check (10419320) fails.
        if mind.posture == Posture::Lie && crate::low_violence::on() {
            mind.posture = Posture::Stagger;
        }
        mind.yaw = mind.float(-std::f32::consts::PI, std::f32::consts::PI);
        mind.curve_max = mind.float(-STUMBLE_CURVE_RATE, STUMBLE_CURVE_RATE).to_radians();
        mind.groan_at = mind.float(0., 6.);
        mind.activity = mind.posture_activity();
        mind
    }
    fn next(&mut self) -> u32 {
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        x
    }
    fn float(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (self.next() as f32 / u32::MAX as f32) * (hi - lo)
    }
    fn int(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo + 1) as u32) as i32
    }
    fn posture_activity(&self) -> &'static str {
        match self.posture {
            Posture::Stagger => "ACT_TERROR_WALK_NEUTRAL",
            Posture::Sit => "ACT_TERROR_SIT_IDLE",
            Posture::Lie => "ACT_TERROR_LIE_IDLE",
        }
    }
    /// Starts a weighted clip of `activity` (SelectWeightedSequence).
    fn begin_alert_clip(&mut self, now: f32, activity: &'static str) {
        let Some((_, clips)) = ALERT_CLIPS.iter().find(|(a, _)| *a == activity) else {
            self.alert_clip = None;
            self.activity = activity;
            return;
        };
        let total: u32 = clips.iter().map(|c| c.2).sum();
        let mut roll = self.int(0, total as i32 - 1) as u32;
        let &(label, length, _) = clips
            .iter()
            .find(|c| {
                if roll < c.2 {
                    true
                } else {
                    roll -= c.2;
                    false
                }
            })
            .unwrap_or(&clips[0]);
        self.activity = label;
        self.alert_clip = Some((label, length, now));
    }
    /// InfectedAlert's clip (104070e0): ahead or behind within 45 degrees of
    /// the stimulus, otherwise left or right, from the injured set when the
    /// alert came from damage.
    pub(crate) fn alert_activity(&self, position: Vec3, toward: Vec3, injured: bool) -> &'static str {
        let to = Vec3::new(toward.x - position.x, 0., toward.z - position.z).normalize_or_zero();
        let ahead = forward(self.yaw).dot(to);
        let left = forward(self.yaw + std::f32::consts::FRAC_PI_2).dot(to);
        match (ahead > 0.7071, ahead < -0.7071, left > 0.7071, injured) {
            (true, _, _, false) => "ACT_TERROR_IDLE_ALERT_AHEAD",
            (true, _, _, true) => "ACT_TERROR_IDLE_ALERT_INJURED_AHEAD",
            (_, true, _, false) => "ACT_TERROR_IDLE_ALERT_BEHIND",
            (_, true, _, true) => "ACT_TERROR_IDLE_ALERT_INJURED_BEHIND",
            (_, _, true, false) => "ACT_TERROR_IDLE_ALERT_LEFT",
            (_, _, true, true) => "ACT_TERROR_IDLE_ALERT_INJURED_LEFT",
            (_, _, _, false) => "ACT_TERROR_IDLE_ALERT_RIGHT",
            (_, _, _, true) => "ACT_TERROR_IDLE_ALERT_INJURED_RIGHT",
        }
    }
    /// Furthest a survivor can be seen from in the current state.
    pub(crate) fn sight_range(&self) -> f32 {
        if matches!(self.state, State::Alert { .. }) {
            VISION_RANGE_ALERT
        } else {
            VISION_RANGE
        }
    }
    /// InfectedStaggerAround::OnContact (10419100): the shamble was blocked.
    /// `wall` is the horizontal normal of a wall it can lean on (vertical,
    /// with support at lean height): it leans against it ("Bumped into a
    /// wall"). Anything else turns it about ("Bumped into something").
    pub(crate) fn bump(&mut self, now: f32, wall: Option<Vec3>) {
        if self.posture != Posture::Stagger || !matches!(self.stagger, Some(Stagger::Walk { .. })) {
            return;
        }
        self.curve = 0.;
        self.curve_max = -self.curve_max;
        let Some(normal) = wall else {
            let clip = TurnKind::About(self.int(0, 2) as usize);
            self.begin_turn(now, clip);
            return;
        };
        // InfectedLeanOnWall::OnStart (10418ac0), in the horizontal plane
        // (x, -z), the same handedness as the yaw convention.
        let n = Vec2::new(normal.x, -normal.z).normalize_or_zero();
        let f = Vec2::new(self.yaw.cos(), self.yaw.sin());
        let right = Vec2::new(self.yaw.sin(), -self.yaw.cos());
        let d = f.dot(n);
        let (activity, face) = if d < -LEAN_DOT {
            ("ACT_TERROR_LEAN_FORWARD_IDLE", -n)
        } else if d > LEAN_DOT {
            ("ACT_TERROR_LEAN_BACKWARD_IDLE", n)
        } else if right.dot(n) > 0. {
            ("ACT_TERROR_LEAN_LEFTWARD_IDLE", Vec2::new(-n.y, n.x))
        } else {
            ("ACT_TERROR_LEAN_RIGHTWARD_IDLE", Vec2::new(n.y, -n.x))
        };
        let until = now + self.float(LEAN_MIN, LEAN_MAX);
        self.stagger = Some(Stagger::Lean { activity, goal: face.y.atan2(face.x), until });
    }
    fn begin_turn(&mut self, now: f32, clip: TurnKind) {
        self.stagger = Some(Stagger::Turn { clip, start: now });
    }
    fn begin_walk(&mut self, now: f32) {
        let clip = self.int(0, WALK_CLIPS.len() as i32 - 1) as usize;
        self.stagger = Some(Stagger::Walk { clip, start: now, until: now + WALK_CLIPS[clip].1 * CLIP_HANDOVER });
    }
    /// InfectedStaggerAround::OnStart (104193a0): half the time a standing
    /// activity first ("Starting with random facing": RandomInt(0, 100)
    /// < 33 face left, <= 65 face right, else about face); otherwise walk.
    fn begin_stagger(&mut self, now: f32) {
        if self.float(0., 100.) < 50. {
            let roll = self.int(0, 100);
            let clip = if roll < 33 {
                TurnKind::Left
            } else if roll <= 65 {
                TurnKind::Right
            } else {
                TurnKind::About(self.int(0, 2) as usize)
            };
            self.begin_turn(now, clip);
        } else {
            self.begin_walk(now);
        }
    }
    /// One tick of a staggering wanderer.
    fn stagger_tick(&mut self, now: f32, dt: f32) -> Intent {
        if self.stagger.is_none() {
            self.begin_stagger(now);
        }
        loop {
            match self.stagger.expect("stagger action") {
                Stagger::Walk { clip, start, until } => {
                    if now >= until {
                        let idle = self.int(0, IDLE_CLIPS.len() as i32 - 1) as usize;
                        self.stagger = Some(Stagger::Stand { clip: idle, start: now, until: now + IDLE_CLIPS[idle].1 * CLIP_HANDOVER });
                        continue;
                    }
                    // InfectedStaggerAround::Update (104185c0): the curve
                    // rate eases toward its random limit.
                    let step = STUMBLE_CURVE_ACCEL.to_radians() * dt;
                    self.curve = if self.curve_max >= 0. {
                        (self.curve + step).min(self.curve_max)
                    } else {
                        (self.curve - step).max(self.curve_max)
                    };
                    self.yaw += self.curve * dt;
                    self.activity = WALK_CLIPS[clip].0;
                    return Intent::Clip { label: self.activity, start };
                }
                Stagger::Stand { clip, start, until } => {
                    if now >= until {
                        self.begin_walk(now);
                        continue;
                    }
                    self.activity = IDLE_CLIPS[clip].0;
                    return Intent::Clip { label: self.activity, start };
                }
                Stagger::Turn { clip, start } => {
                    let c = clip.clip();
                    if (now - start) / c.length >= c.end {
                        self.begin_walk(now);
                        continue;
                    }
                    self.activity = c.label;
                    return Intent::Clip { label: self.activity, start };
                }
                Stagger::Lean { activity, goal, until } => {
                    // InfectedLeanOnWall::Update (10418fd0): a backward lean
                    // is done; any other ends in an about face.
                    if now >= until {
                        if activity == "ACT_TERROR_LEAN_BACKWARD_IDLE" {
                            self.begin_walk(now);
                        } else {
                            let clip = TurnKind::About(self.int(0, 2) as usize);
                            self.begin_turn(now, clip);
                        }
                        continue;
                    }
                    self.yaw = turn_toward(self.yaw, goal, LEAN_ALIGN_SPEED.to_radians() * dt);
                    self.activity = activity;
                    return Intent::Idle { activity, yaw: self.yaw };
                }
            }
        }
    }
    pub(crate) fn provoke(&mut self) {
        self.state = State::Attack;
    }
    pub(crate) fn attacking(&self) -> bool {
        self.state == State::Attack
    }
    /// Vision range by state (FUN_103fcd00): 500 calm, 1500 alert.
    fn sees(&self, position: Vec3, threat: &Threat, alert: bool) -> bool {
        let to = threat.position - position;
        let distance = to.length() / UNIT;
        if !threat.visible {
            return false;
        }
        if distance <= NOTICE_NEAR_RANGE {
            return true;
        }
        let range = if alert { VISION_RANGE_ALERT } else { VISION_RANGE };
        distance <= range
            && forward(self.yaw).dot((to * Vec3::new(1., 0., 1.)).normalize_or_zero())
                >= VISION_HALF_FOV_COS
    }
    /// InfectedWander::OnSound (1041a770): gunfire within z_hear_gunfire_range
    /// times the posture's attention; a survivor running for 2 s within
    /// z_hear_runner_near_range, or within the far range heading at us.
    fn hears(&self, position: Vec3, threat: &Threat) -> bool {
        let to = position - threat.position;
        let distance = to.length() / UNIT;
        if threat.fired && distance <= HEAR_GUNFIRE_RANGE * self.posture.attention() {
            return true;
        }
        if threat.running > RUNNER_TIME {
            if distance < HEAR_RUNNER_NEAR_RANGE {
                return true;
            }
            if distance < HEAR_RUNNER_FAR_RANGE {
                let away = (to * Vec3::new(1., 0., 1.)).normalize_or_zero();
                return away.dot(threat.forward) > -RUNNER_FAR_DOT;
            }
        }
        false
    }
    fn nearest<'a>(position: Vec3, threats: &'a [Threat]) -> Option<&'a Threat> {
        threats.iter().min_by(|a, b| {
            a.position
                .distance_squared(position)
                .total_cmp(&b.position.distance_squared(position))
        })
    }
    /// One behaviour tick. `sound` receives the voice to play.
    pub(crate) fn think(&mut self, senses: &Senses, dt: f32, sound: &mut Option<&'static str>) -> Intent {
        let now = senses.clock;
        let position = senses.position;
        self.position = position;
        if senses.injured {
            // A shambler that is hurt turns InfectedAlert with the injured
            // set facing the damage (retail, 13 November: 10 bullet damage
            // from 58 to 600 units always played IDLE_ALERT_INJURED_* and
            // stayed alert); anyone else already alert attacks.
            match (self.state, self.posture, Self::nearest(position, senses.threats)) {
                (State::Wander { .. }, Posture::Stagger, Some(t)) => {
                    self.begin_alert(now, t.position);
                    if let State::Alert { ref mut injured, .. } = self.state {
                        *injured = true;
                    }
                    let activity = self.alert_activity(position, t.position, true);
                    self.begin_alert_clip(now, activity);
                }
                (State::Calming { .. } | State::Rising { .. }, _, _) | (State::Wander { .. }, _, _) => {
                    self.state = State::Attack;
                }
                (State::Alert { .. }, _, _) | (State::Attack, _, _) => self.state = State::Attack,
            }
        }
        loop {
            match self.state {
                State::Attack => return Intent::Attack,
                State::Wander { notice } => {
                    // "Zombie.Wander" (standing) or "Zombie.Sleeping" every
                    // RandomFloat(4, 6) seconds.
                    if now >= self.groan_at {
                        self.groan_at = now + self.float(4., 6.);
                        *sound = Some(if self.posture == Posture::Stagger {
                            "Zombie.Wander"
                        } else {
                            "Zombie.Sleeping"
                        });
                    }
                    // A panic or mob (retail director_force_panic_event on
                    // c1m1_hotel, 12 November): every wanderer, idle or
                    // shambling, broke into its run on the same 0.1 s sample
                    // with no notice delay and no alert or acquire pose.
                    if senses.mob {
                        if let Some(t) = Self::nearest(position, senses.threats) {
                            self.begin_alert(now, t.position);
                            continue;
                        }
                    }
                    match notice {
                        Some((at, toward)) if now >= at => {
                            self.begin_alert(now, toward);
                            continue;
                        }
                        Some(_) => {}
                        None => {
                            let noticed = if senses.mob {
                                Self::nearest(position, senses.threats)
                            } else {
                                senses.threats.iter().find(|t| {
                                    self.sees(position, t, false) || self.hears(position, t)
                                })
                            };
                            if let Some(t) = noticed {
                                let delay = self.float(0.3, 1.) * self.posture.notice_scale();
                                self.state = State::Wander { notice: Some((now + delay, t.position)) };
                            }
                        }
                    }
                    if self.posture == Posture::Stagger {
                        return self.stagger_tick(now, dt);
                    }
                    self.activity = self.posture_activity();
                    return Intent::Idle { activity: self.activity, yaw: self.yaw };
                }
                State::Rising { until, toward } => {
                    if now >= until {
                        self.posture = Posture::Stagger;
                        self.begin_alert(now, toward);
                        continue;
                    }
                    return Intent::Idle { activity: self.activity, yaw: self.yaw };
                }
                State::Alert { toward, calm_at, acquire_at, injured } => {
                    if senses.mob {
                        self.state = State::Attack;
                        continue;
                    }
                    // "Zombie.Alert" every RandomFloat(2, 4) seconds.
                    if now >= self.groan_at {
                        self.groan_at = now + self.float(2., 4.);
                        *sound = Some("Zombie.Alert");
                    }
                    let seen = senses
                        .threats
                        .iter()
                        .filter(|t| self.sees(position, t, true))
                        .min_by(|a, b| {
                            a.position
                                .distance_squared(position)
                                .total_cmp(&b.position.distance_squared(position))
                        })
                        .copied();
                    let mut look = toward;
                    let acquire_at = match (seen, acquire_at) {
                        (Some(t), None) => {
                            let distance = t.position.distance(position) / UNIT;
                            let along = ((distance - ACQUIRE_NEAR_RANGE)
                                / (ACQUIRE_FAR_RANGE - ACQUIRE_NEAR_RANGE))
                                .clamp(0., 1.);
                            let time = ACQUIRE_NEAR_TIME + (ACQUIRE_FAR_TIME - ACQUIRE_NEAR_TIME) * along;
                            look = t.position;
                            Some(now + time * self.float(1. - ACQUIRE_VARIANCE, 1. + ACQUIRE_VARIANCE))
                        }
                        (Some(t), Some(at)) => {
                            look = t.position;
                            if now >= at {
                                self.state = State::Attack;
                                continue;
                            }
                            Some(at)
                        }
                        // Losing sight resets the acquire clock.
                        (None, _) => None,
                    };
                    // The alert ends on its RandomFloat(5, 15) timer or when its
                    // clip finishes (a startle clip lasts about 1.5 s).
                    let finished = self
                        .alert_clip
                        .is_some_and(|(label, length, start)| {
                            !label.starts_with("Idle_Acquire") && now - start >= length - CLIP_FADE_OUT
                        });
                    if now >= calm_at || (acquire_at.is_none() && finished) {
                        self.state = State::Calming { until: now + ALERT_TO_NEUTRAL };
                        continue;
                    }
                    self.state = State::Alert { toward: look, calm_at, acquire_at, injured };
                    match acquire_at {
                        Some(_) => {
                            // InfectedAlert::Update (10407350): acquiring sets
                            // ACT_TERROR_IDLE_ACQUIRE once and only aims the
                            // head ("Watch our victim"); the clip moves the body.
                            let acquiring = self.alert_clip.is_some_and(|(l, _, _)| l.starts_with("Idle_Acquire"));
                            if !acquiring {
                                let (label, length) = ACQUIRE_CLIPS[self.int(0, 2) as usize];
                                self.activity = label;
                                self.alert_clip = Some((label, length, now));
                            }
                        }
                        None => {
                            // Sight lost while acquiring: plain ACT_TERROR_IDLE_ALERT.
                            if self.alert_clip.is_some_and(|(l, _, _)| l.starts_with("Idle_Acquire")) {
                                self.alert_clip = None;
                                self.activity = "ACT_TERROR_IDLE_ALERT";
                            }
                        }
                    }
                    if let Some((label, _, start)) = self.alert_clip {
                        return Intent::Clip { label, start };
                    }
                    return Intent::Idle { activity: self.activity, yaw: self.yaw };
                }
                State::Calming { until } => {
                    if now >= until {
                        self.state = State::Wander { notice: None };
                        self.groan_at = now + self.float(4., 6.);
                        continue;
                    }
                    self.activity = "ACT_TERROR_ALERT_TO_NEUTRAL";
                    return Intent::Idle { activity: self.activity, yaw: self.yaw };
                }
            }
        }
    }
    /// Sitting and lying commons get up first (the posture actions' alert
    /// exits); standing ones turn straight to InfectedAlert, which stays
    /// alert RandomFloat(8, 12) seconds before calming down.
    fn begin_alert(&mut self, now: f32, toward: Vec3) {
        // Leaving the wander: its stagger starts over (OnStart) afterwards.
        self.stagger = None;
        match self.posture {
            Posture::Sit => {
                self.activity = "ACT_TERROR_SIT_TO_STAND_ALERT";
                self.state = State::Rising { until: now + SIT_TO_STAND, toward };
            }
            Posture::Lie => {
                self.activity = "ACT_TERROR_LIE_TO_STAND_ALERT";
                self.state = State::Rising { until: now + LIE_TO_STAND, toward };
            }
            Posture::Stagger => {
                // InfectedAlert::OnStart's clip choice (104070e0) arms the
                // calm-down timer at RandomFloat(5, 15) seconds.
                let calm = self.float(5., 15.);
                let directional = self.alert_activity(self.position, toward, false);
                self.begin_alert_clip(now, directional);
                self.groan_at = now;
                self.state = State::Alert { toward, calm_at: now + calm, acquire_at: None, injured: false };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn threat(position: Vec3) -> Threat {
        Threat { position, forward: Vec3::X, visible: true, running: 0., fired: false }
    }
    fn run(mind: &mut Mind, threats: &[Threat], from: f32, to: f32) -> (Intent, f32) {
        let mut t = from;
        let mut sound = None;
        let mut intent = Intent::Attack;
        while t < to {
            let senses = Senses { clock: t, position: Vec3::ZERO, threats, mob: false, injured: false };
            intent = mind.think(&senses, 0.1, &mut sound);
            if intent == Intent::Attack {
                return (intent, t);
            }
            t += 0.1;
        }
        (intent, t)
    }
    #[test]
    fn retail_posture_odds_are_fifteen_lie_fifteen_sit_rest_stagger() {
        let counts = (0..=100).fold([0; 3], |mut c, r| {
            c[Posture::roll(r) as usize] += 1;
            c
        });
        assert_eq!(counts, [71, 15, 15]);
    }
    #[test]
    fn a_wanderer_ignores_survivors_out_of_sight_and_behind_it() {
        let mut mind = Mind::wanderer(7, Some(Posture::Sit));
        mind.yaw = 0.;
        // 300 units behind: inside vision range but outside the view cone.
        let behind = threat(-Vec3::X * 300. * UNIT);
        let (intent, _) = run(&mut mind, &[behind], 0., 10.);
        assert_eq!(intent, Intent::Idle { activity: "ACT_TERROR_SIT_IDLE", yaw: 0. });
        // 600 units ahead: beyond z_vision_range.
        let far = threat(Vec3::X * 600. * UNIT);
        let (intent, _) = run(&mut mind, &[far], 10., 20.);
        assert!(matches!(intent, Intent::Idle { activity: "ACT_TERROR_SIT_IDLE", .. }));
    }
    #[test]
    fn a_seen_survivor_is_noticed_then_acquired_then_attacked() {
        let mut mind = Mind::wanderer(3, Some(Posture::Stagger));
        mind.yaw = 0.;
        let ahead = threat(Vec3::X * 300. * UNIT);
        let (intent, at) = run(&mut mind, &[ahead], 0., 20.);
        assert_eq!(intent, Intent::Attack);
        // Notice 0.3-1 s, then acquire 0.5 + 4.5 * 100/2300 s +-25%.
        assert!((0.6..=2.).contains(&at), "attacked at {at}");
    }
    #[test]
    fn a_lying_common_gets_up_before_it_turns_alert() {
        let mut mind = Mind::wanderer(11, Some(Posture::Lie));
        mind.yaw = 0.;
        let near = threat(Vec3::X * 50. * UNIT);
        let mut sound = None;
        let mut saw_rise = false;
        for i in 0..80 {
            let senses = Senses { clock: i as f32 * 0.1, position: Vec3::ZERO, threats: &[near], mob: false, injured: false };
            mind.think(&senses, 0.1, &mut sound);
            saw_rise |= mind.activity == "ACT_TERROR_LIE_TO_STAND_ALERT";
        }
        assert!(saw_rise);
        assert!(mind.attacking());
        assert_eq!(mind.posture, Posture::Stagger);
    }
    #[test]
    fn gunfire_is_heard_closer_by_sitting_and_lying_commons() {
        let mut shot = threat(-Vec3::X * 100. * UNIT);
        shot.visible = false;
        shot.fired = true;
        let stand = Mind::wanderer(1, Some(Posture::Stagger));
        let sit = Mind::wanderer(1, Some(Posture::Sit));
        assert!(stand.hears(Vec3::ZERO, &shot));
        assert!(!sit.hears(Vec3::ZERO, &shot));
        shot.position = -Vec3::X * 60. * UNIT;
        assert!(sit.hears(Vec3::ZERO, &shot));
    }
    #[test]
    fn runners_are_heard_after_two_seconds_and_farther_when_heading_here() {
        let mind = Mind::wanderer(1, Some(Posture::Stagger));
        let mut runner = threat(Vec3::X * 600. * UNIT);
        runner.visible = false;
        runner.running = 3.;
        runner.forward = Vec3::X;
        assert!(!mind.hears(Vec3::ZERO, &runner), "running away, beyond the near range");
        runner.forward = -Vec3::X;
        assert!(mind.hears(Vec3::ZERO, &runner), "running straight at the common");
        runner.position = Vec3::X * 400. * UNIT;
        runner.running = 1.;
        assert!(!mind.hears(Vec3::ZERO, &runner), "not running long enough");
    }
    #[test]
    fn being_hurt_or_a_mob_turns_a_wanderer_hostile() {
        let mut mind = Mind::wanderer(5, Some(Posture::Sit));
        let mut sound = None;
        let senses = Senses { clock: 0., position: Vec3::ZERO, threats: &[], mob: false, injured: true };
        assert_eq!(mind.think(&senses, 0.1, &mut sound), Intent::Attack);
        // A hurt shambler turns alert with the injured set toward the shot,
        // and a second hit while alert attacks.
        let mut shambler = Mind::wanderer(5, Some(Posture::Stagger));
        shambler.yaw = 0.;
        let shooter = threat(Vec3::NEG_X * 500. * UNIT);
        let senses = Senses { clock: 0., position: Vec3::ZERO, threats: &[shooter], mob: false, injured: true };
        assert_ne!(shambler.think(&senses, 0.1, &mut sound), Intent::Attack);
        assert!(matches!(shambler.state, State::Alert { injured: true, .. }));
        assert_eq!(shambler.activity, "Idle_Alert_Injured_Behind_01");
        assert_eq!(shambler.think(&senses, 0.1, &mut sound), Intent::Attack);
        let mut mind = Mind::wanderer(5, Some(Posture::Stagger));
        mind.yaw = std::f32::consts::PI;
        let hidden = Threat { visible: false, ..threat(Vec3::X * 1000. * UNIT) };
        let mut t = 0.;
        let mut intent = Intent::Walk { yaw: 0. };
        while t < 5. && intent != Intent::Attack {
            let senses = Senses { clock: t, position: Vec3::ZERO, threats: &[hidden], mob: true, injured: false };
            intent = mind.think(&senses, 0.1, &mut sound);
            t += 0.1;
        }
        assert_eq!(intent, Intent::Attack);
    }
    #[test]
    fn an_alert_common_that_never_sees_anyone_calms_back_down() {
        let mut mind = Mind::wanderer(9, Some(Posture::Stagger));
        mind.begin_alert(0., Vec3::X);
        let (_, _) = run(&mut mind, &[], 0., 17.);
        // Back to InfectedStaggerAround: a walk clip or its opening turn.
        assert!(mind.stagger.is_some());
        assert!(matches!(mind.state, State::Wander { notice: None }));
    }
    #[test]
    fn a_shambler_leans_on_a_wall_and_turns_about_after_anything_else() {
        let mut mind = Mind::wanderer(4, Some(Posture::Stagger));
        mind.yaw = 0.;
        mind.stagger = Some(Stagger::Walk { clip: 0, start: 0., until: 100. });
        // Facing +x into a wall whose normal points back at it: forward lean,
        // turning to face the wall, held 5 to 20 s.
        mind.bump(1., Some(Vec3::NEG_X));
        let Some(Stagger::Lean { activity, goal, until }) = mind.stagger else { panic!("{:?}", mind.stagger) };
        assert_eq!(activity, "ACT_TERROR_LEAN_FORWARD_IDLE");
        assert!(goal.abs() < 1e-4 && (6. ..=21.).contains(&until));
        // A wall on the common's left (normal pointing to its right).
        mind.stagger = Some(Stagger::Walk { clip: 0, start: 0., until: 100. });
        mind.bump(1., Some(Vec3::Z));
        assert!(matches!(mind.stagger, Some(Stagger::Lean { activity: "ACT_TERROR_LEAN_LEFTWARD_IDLE", .. })));
        // Anything else: an about face that turns the body with its clip.
        mind.stagger = Some(Stagger::Walk { clip: 0, start: 0., until: 100. });
        mind.bump(1., None);
        assert!(matches!(mind.stagger, Some(Stagger::Turn { clip: TurnKind::About(_), .. })));
        let (intent, _) = run(&mut mind, &[], 1., 2.);
        assert!(matches!(intent, Intent::Clip { label, .. } if label.starts_with("Idle_Neutral_Turn_Around")), "{intent:?}");
        // The turn hands back to a walk clip once it has played out.
        let (intent, _) = run(&mut mind, &[], 2., 5.);
        assert!(matches!(intent, Intent::Clip { label, .. } if label.starts_with("Walk_Neutral")), "{intent:?}");
    }
}
