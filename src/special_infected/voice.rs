//! Player special-infected voice and lifecycle sounds. Follows the retail
//! server.dll player-infected code (build 10097): idle voices FUN_10347670
//! (Hunter FUN_10347b70, Jockey FUN_103485b0), voice timer FUN_10347590,
//! pain FUN_10306810, death FUN_10336010, become ghost/real FUN_103269d0 /
//! FUN_10322310, alert mood FUN_1031d9c0/FUN_10301090, attack-hit
//! FUN_10306120. Evidence: local-research/si-sounds/NOTES.md.
use super::{Hunter, SpecialClass};
use crate::{
    collision::{Collision, UNIT},
    game::Body,
    game_audio::{MapSound, Queue, Request},
    game_state::{Match, Squad},
};
use bevy::prelude::*;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};
/// DAT_105e188c: minimum seconds between pain voices.
pub const PAIN_INTERVAL: f32 = 0.7;
/// DAT_105d50cc: pad after a voice played with no explicit delay.
pub const VOICE_PAD: f32 = 0.2;
/// DAT_105d7844: mood (survivor sighting) update interval.
const SIGHT_INTERVAL: f32 = 0.25;
/// A survivor seen within this many seconds makes the player alert.
const ALERT_SEEN: f32 = 1.;
/// DAT_105d2e20: alert returns to normal after this long unseen.
const ALERT_HOLD: f32 = 5.;
/// z_pounce_silence_range: crouched Hunter warns when a survivor is closer.
pub const POUNCE_SILENCE_RANGE: f32 = 500.;
/// DAT_105daad4: survivor must be this far inside the Hunter's view cone.
pub const WARN_CONE: f32 = 0.8;
/// DAT_105d4b38: repeated pounce launches within this window stay silent.
pub const POUNCE_REPEAT: f32 = 1.5;
/// Fallback when a voice wave's length is not known yet.
const DEFAULT_LENGTH: f32 = 1.5;
const GURGLE_SOUND: usize = usize::MAX - 0xb00e;
#[derive(Default)]
pub(crate) struct VoiceState {
    clock: f32,
    /// m_voiceTimer: idle and alert voices wait for this.
    next: f32,
    pain_next: f32,
    pain_serial: u64,
    alert: bool,
    seen_at: Option<f32>,
    sight: f32,
    last_ghost: Option<bool>,
    was_dead: bool,
    gurgle: bool,
    riding: bool,
    pounce_at: Option<f32>,
    claw_serial: u64,
    claw_seen: u64,
    shred_clock: Option<f32>,
    seed: u32,
}
impl VoiceState {
    fn random(&mut self, low: f32, high: f32) -> f32 {
        if self.seed == 0 {
            self.seed = 0x9e37_79b9 ^ self.clock.to_bits();
        }
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        low + (high - low) * (self.seed as f32 / u32::MAX as f32)
    }
}
static LENGTHS: OnceLock<Mutex<HashMap<String, f32>>> = OnceLock::new();
fn length(name: &str) -> f32 {
    let key = name.split('#').next().unwrap_or(name).to_ascii_lowercase();
    LENGTHS
        .get()
        .and_then(|l| l.lock().ok()?.get(&key).copied())
        .unwrap_or(DEFAULT_LENGTH)
}
/// Seconds of PCM/ADPCM audio in a RIFF WAVE (fact chunk when present).
pub fn wave_seconds(bytes: &[u8]) -> Option<f32> {
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return None;
    }
    let (mut rate, mut byte_rate, mut frames, mut data) = (0u32, 0u32, None, None);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        let body = bytes.get(at + 8..(at + 8 + size).min(bytes.len()))?;
        match &bytes[at..at + 4] {
            b"fmt " if body.len() >= 12 => {
                rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
                byte_rate = u32::from_le_bytes(body[8..12].try_into().ok()?);
            }
            b"fact" if body.len() >= 4 => {
                frames = Some(u32::from_le_bytes(body[0..4].try_into().ok()?));
            }
            b"data" => data = Some(size as u32),
            _ => {}
        }
        at += 8 + size + size % 2;
    }
    match (frames, data) {
        (Some(frames), _) if rate > 0 => Some(frames as f32 / rate as f32),
        (_, Some(data)) if byte_rate > 0 => Some(data as f32 / byte_rate as f32),
        _ => None,
    }
}
/// Names whose length drives the voice timer (voices played with delay 0).
const TIMED: &[&str] = &[
    "SmokerZombie.TongueAttack",
    "SmokerZombie.Attack",
    "BoomerZombie.Attack",
    "BoomerZombie.Voice",
    "HunterZombie.Pounce.shred",
    "PlayerZombie.Attack",
    "SpitterZombie.Spit",
    "ChargerZombie.Charge",
    "HulkZombie.Breathe",
    "HulkZombie.Yell",
    "SmokerZombie.Pain",
    "SmokerZombie.PainShort",
    "BoomerZombie.Pain",
    "BoomerZombie.PainShort",
    "HunterZombie.Pain",
    "HunterZombie.PainShort",
    "SpitterZombie.Pain",
    "SpitterZombie.PainShort",
    "JockeyZombie.Pain",
    "JockeyZombie.PainShort",
    "ChargerZombie.Pain",
];
/// Reads the installed wave lengths once, off the main thread.
fn load_lengths(root: std::path::PathBuf) {
    if LENGTHS.set(Mutex::new(HashMap::new())).is_err() {
        return;
    }
    std::thread::spawn(move || {
        let Ok(mut assets) = crate::source_assets::SourceAssets::new(&root, vec![]) else {
            return;
        };
        let Ok(registry) = crate::source_sound::Registry::load(&mut assets) else {
            return;
        };
        for name in TIMED {
            let key = name.to_ascii_lowercase();
            let Some(definition) = registry.definitions.get(&key) else {
                continue;
            };
            let longest = definition
                .waves
                .iter()
                .filter_map(|w| assets.read(&w.replace("$gender", "male")).ok())
                .filter_map(|b| wave_seconds(&b))
                .fold(0f32, f32::max);
            if longest > 0. {
                if let Some(lengths) = LENGTHS.get() {
                    lengths.lock().unwrap().insert(key, longest);
                }
            }
        }
    });
}
/// FUN_10347590: emit a voice and restart the voice timer. `delay > 0`
/// holds for that long, `delay == 0` for the sound plus `VOICE_PAD`, and a
/// negative delay leaves the timer alone.
pub(crate) fn speak(hunter: &mut Hunter, sounds: &mut Queue, name: &str, delay: f32) {
    sounds.play(name, None);
    let v = &mut hunter.voice;
    if delay > 0. {
        v.next = v.clock + delay;
    } else if delay == 0. {
        v.next = v.clock + length(name) + VOICE_PAD;
    }
}
/// Keep idle voices quiet for `seconds` (an ability sound is playing).
pub(crate) fn hold(hunter: &mut Hunter, seconds: f32) {
    let v = &mut hunter.voice;
    v.next = v.next.max(v.clock + seconds);
}
/// Claw/punch modules call this on each swing for the retail attack voice.
pub(crate) fn clawed(hunter: &mut Hunter) {
    hunter.voice.claw_serial = hunter.voice.claw_serial.wrapping_add(1);
}
/// Retail skips a second pounce yell within `POUNCE_REPEAT` (wall rebounds).
pub(crate) fn pounce_voice(hunter: &mut Hunter, sounds: &mut Queue) {
    let now = hunter.voice.clock;
    if hunter
        .voice
        .pounce_at
        .is_none_or(|at| now - at >= POUNCE_REPEAT)
    {
        sounds.play("HunterZombie.Pounce", None);
        hunter.voice.pounce_at = Some(now);
    }
}
/// Class voice for a hurt infected player (FUN_10306810). Bullet and club
/// damage use the short variant; the Charger and Tank have only one.
pub fn pain(class: SpecialClass, short: bool) -> &'static str {
    match (class, short) {
        (SpecialClass::Charger, _) => "ChargerZombie.Pain",
        (SpecialClass::Tank, _) => "HulkZombie.Pain",
        (SpecialClass::Smoker, false) => "SmokerZombie.Pain",
        (SpecialClass::Smoker, true) => "SmokerZombie.PainShort",
        (SpecialClass::Boomer, false) => "BoomerZombie.Pain",
        (SpecialClass::Boomer, true) => "BoomerZombie.PainShort",
        (SpecialClass::Hunter, false) => "HunterZombie.Pain",
        (SpecialClass::Hunter, true) => "HunterZombie.PainShort",
        (SpecialClass::Spitter, false) => "SpitterZombie.Pain",
        (SpecialClass::Spitter, true) => "SpitterZombie.PainShort",
        (SpecialClass::Jockey, false) => "JockeyZombie.Pain",
        (SpecialClass::Jockey, true) => "JockeyZombie.PainShort",
    }
}
/// Death voices (FUN_10336010 / FUN_1032e7a0). The Boomer's Detonate is
/// played by its explosion at the body.
pub fn death(class: SpecialClass) -> &'static [&'static str] {
    match class {
        SpecialClass::Smoker => &["SmokerZombie.Death", "SmokerZombie.Explode"],
        SpecialClass::Boomer => &[],
        SpecialClass::Hunter => &["HunterZombie.Death"],
        SpecialClass::Spitter => &["SpitterZombie.Death"],
        SpecialClass::Jockey => &["JockeyZombie.Death"],
        SpecialClass::Charger => &["ChargerZombie.Death"],
        SpecialClass::Tank => &["HulkZombie.Die"],
    }
}
/// One-off voice when the player turns alert (FUN_10301090).
pub fn alert(class: SpecialClass) -> Option<&'static str> {
    match class {
        SpecialClass::Smoker => Some("SmokerZombie.Recognize"),
        SpecialClass::Charger => Some("ChargerZombie.Recognize"),
        SpecialClass::Spitter => Some("SpitterZombie.Recognize"),
        SpecialClass::Boomer => Some("BoomerZombie.Rage"),
        SpecialClass::Tank => Some("HulkZombie.Yell"),
        SpecialClass::Hunter | SpecialClass::Jockey => None,
    }
}
/// Attack voice while clawing (second block of FUN_10347670).
pub fn attack(class: SpecialClass) -> Option<&'static str> {
    match class {
        SpecialClass::Smoker => Some("SmokerZombie.Attack"),
        SpecialClass::Boomer => Some("BoomerZombie.Attack"),
        SpecialClass::Hunter => Some("PlayerZombie.Attack"),
        _ => None,
    }
}
fn class_voice(class: SpecialClass, name: &str) -> String {
    if class == SpecialClass::Boomer {
        super::boomer::voice(name)
    } else {
        name.into()
    }
}
/// Idle voice choice for one elapsed voice timer: (sound, delay passed to
/// `speak`, timer override). Hunter and Jockey use their own functions.
pub(crate) struct Idle {
    pub sound: &'static str,
    pub delay: f32,
    pub timer: Option<f32>,
}
pub(crate) fn idle(
    class: SpecialClass,
    alert: bool,
    holding: bool,
    busy: bool,
    roll: &mut impl FnMut(f32, f32) -> f32,
) -> Option<Idle> {
    let timed = |sound, delay| {
        Some(Idle {
            sound,
            delay,
            timer: None,
        })
    };
    match class {
        SpecialClass::Smoker if holding => timed("SmokerZombie.Attack", roll(1., 2.)),
        SpecialClass::Smoker => timed("SmokerZombie.Breathe", roll(4., 6.)),
        SpecialClass::Boomer if alert => Some(Idle {
            sound: "BoomerZombie.Voice",
            delay: 0.,
            timer: Some(roll(1., 2.)),
        }),
        SpecialClass::Boomer => timed("BoomerZombie.Groan", roll(2., 3.)),
        SpecialClass::Spitter => timed("SpitterZombie.Voice", roll(3., 5.)),
        SpecialClass::Charger if busy => None,
        SpecialClass::Charger => timed("ChargerZombie.Voice", roll(3., 5.)),
        SpecialClass::Tank if alert => Some(Idle {
            sound: "HulkZombie.Yell",
            delay: 0.,
            timer: Some(roll(4., 6.)),
        }),
        SpecialClass::Tank => {
            if roll(0., 100.) <= 24. {
                timed("HulkZombie.Growl", roll(1., 2.))
            } else {
                Some(Idle {
                    sound: "HulkZombie.Breathe",
                    delay: 0.,
                    timer: Some(roll(1., 2.)),
                })
            }
        }
        SpecialClass::Hunter | SpecialClass::Jockey => None,
    }
}
fn stop_gurgle(
    v: &mut VoiceState,
    commands: &mut Commands,
    sounds: &mut Queue,
    playing: &Query<(Entity, &MapSound)>,
) {
    if std::mem::take(&mut v.gurgle) {
        sounds.1.push(GURGLE_SOUND);
        for (entity, sound) in playing {
            if sound.0 == GURGLE_SOUND {
                commands.entity(entity).despawn();
            }
        }
    }
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn tick(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    keys: Res<ButtonInput<KeyCode>>,
    collision: Res<Collision>,
    mut state: ResMut<Match>,
    mut hunter: ResMut<Hunter>,
    body: Single<&Body>,
    squad: Res<Squad>,
    mut sounds: ResMut<Queue>,
    extra: (
        Query<(Entity, &MapSound)>,
        Option<Res<crate::game_ui::HudSource>>,
        Option<Res<crate::l4d2r_animation::AnimationLibrary>>,
        Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    ),
) {
    let (playing, source, library, logic) = extra;
    if let Some(source) = source {
        if LENGTHS.get().is_none() {
            load_lengths(source.0.clone());
        }
    }
    let class = state.special_class;
    if !state.infected {
        stop_gurgle(&mut hunter.voice, &mut commands, &mut sounds, &playing);
        hunter.voice = VoiceState::default();
        return;
    }
    if state.paused || logic.is_some_and(|l| l.active) {
        return;
    }
    let dt = time.delta_secs();
    hunter.voice.clock += dt;
    let ghost = hunter.ghost && !state.dead;
    // Ghost transitions: BecomeGhost entering spawn mode, BecomeReal (and the
    // Boomer's indigestion loop) on materializing. The Tank never ghosts.
    let previous = hunter.voice.last_ghost.replace(ghost);
    if class != SpecialClass::Tank {
        if ghost && previous != Some(true) {
            sounds.play("PlayerZombie.BecomeGhost", None);
        }
        if previous == Some(true) && !ghost && !state.dead {
            sounds.play("PlayerZombie.BecomeReal", None);
            hunter.voice.next = hunter.voice.clock;
            if class == SpecialClass::Boomer {
                sounds.0.push_back(Request {
                    name: "BoomerZombie.Gurgle".into(),
                    position: None,
                    looping: true,
                    gain: 1.,
                    map_entity: Some(GURGLE_SOUND),
                    soundlevel: None,
                });
                hunter.voice.gurgle = true;
            }
        }
    }
    if state.dead && !hunter.voice.was_dead {
        for name in death(class) {
            sounds.play(*name, None);
        }
    }
    hunter.voice.was_dead = state.dead;
    if state.dead || ghost {
        stop_gurgle(&mut hunter.voice, &mut commands, &mut sounds, &playing);
        hunter.voice.alert = false;
        hunter.voice.seen_at = None;
        hunter.voice.pain_serial = state.damage_serial;
        return;
    }
    // Pain: rate limited, short for bullets/clubs; survivor hits also give
    // the attacker-hit slice (not for the Charger).
    if hunter.voice.pain_serial != state.damage_serial {
        hunter.voice.pain_serial = state.damage_serial;
        let short = std::mem::take(&mut state.damage_short);
        if short && class != SpecialClass::Charger {
            sounds.play("PlayerZombie.AttackHit", None);
        }
        if hunter.voice.clock >= hunter.voice.pain_next {
            hunter.voice.pain_next = hunter.voice.clock + PAIN_INTERVAL;
            let name = class_voice(class, pain(class, short));
            let delay = if class == SpecialClass::Tank {
                PAIN_INTERVAL
            } else {
                0.
            };
            speak(&mut hunter, &mut sounds, &name, delay);
        }
    }
    // Mood: alert while a survivor is in sight, normal 5 s after losing it.
    let eye = body.feet + Vec3::Y * 1.5;
    hunter.voice.sight -= dt;
    if hunter.voice.sight <= 0. {
        hunter.voice.sight = SIGHT_INTERVAL;
        let visible = squad.0.iter().any(|m| {
            !m.state.dead
                && collision
                    .trace(eye, m.position + Vec3::Y * 1.2, Vec3::ZERO)
                    .fraction
                    >= 1.
        });
        if visible {
            hunter.voice.seen_at = Some(hunter.voice.clock);
        }
    }
    let since = hunter
        .voice
        .seen_at
        .map_or(f32::INFINITY, |at| hunter.voice.clock - at);
    if !hunter.voice.alert && since < ALERT_SEEN {
        hunter.voice.alert = true;
        if let Some(name) = alert(class) {
            let delay = hunter.voice.random(3., 5.);
            speak(&mut hunter, &mut sounds, &class_voice(class, name), delay);
        }
    } else if hunter.voice.alert && since > ALERT_HOLD {
        hunter.voice.alert = false;
    }
    // Hunter pin slices: the retail Melee_Pounce animation's sound events.
    let shredding =
        class == SpecialClass::Hunter && hunter.pin.is_some() && hunter.damage_clock <= 0.5;
    if shredding {
        if let Some((length, events)) =
            library.and_then(|l| l.sequence_sounds(super::HUNTER_MODEL, "Melee_Pounce"))
        {
            let before = hunter.voice.shred_clock.unwrap_or(0.);
            let after = before + dt;
            for (at, name) in &events {
                let (a, b) = (before.rem_euclid(length), after.rem_euclid(length));
                let crossed = if b < a {
                    *at >= a || *at < b
                } else {
                    *at >= a && *at < b
                };
                if crossed {
                    sounds.play(name.clone(), None);
                }
            }
            hunter.voice.shred_clock = Some(after);
        }
    } else {
        hunter.voice.shred_clock = None;
    }
    // Dismounting restarts the laugh at once (10313220 resets the timer).
    let riding = class == SpecialClass::Jockey && hunter.pin.is_some();
    if hunter.voice.riding && !riding {
        hunter.voice.next = hunter.voice.clock;
    }
    hunter.voice.riding = riding;
    let ready = hunter.voice.clock >= hunter.voice.next;
    // Attack voice while clawing.
    if hunter.voice.claw_serial != hunter.voice.claw_seen {
        hunter.voice.claw_seen = hunter.voice.claw_serial;
        if ready {
            if let Some(name) = attack(class) {
                speak(&mut hunter, &mut sounds, &class_voice(class, name), 0.);
                return;
            }
        }
    }
    if !ready {
        return;
    }
    match class {
        SpecialClass::Hunter => {
            let forward = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
            if hunter.pin.is_some() {
                if shredding {
                    speak(&mut hunter, &mut sounds, "HunterZombie.Pounce.shred", 0.);
                } else {
                    let delay = hunter.voice.random(3., 4.5);
                    speak(&mut hunter, &mut sounds, "HunterZombie.Warn", delay);
                }
            } else if keys.pressed(KeyCode::ControlLeft)
                && since < ALERT_SEEN
                && hunter_warns(eye, forward, &squad)
            {
                let delay = hunter.voice.random(3., 4.5);
                speak(&mut hunter, &mut sounds, "HunterZombie.Warn", delay);
            } else {
                let delay = hunter.voice.random(2., 3.5);
                speak(&mut hunter, &mut sounds, "HunterZombie.Growl", delay);
            }
        }
        SpecialClass::Jockey => {
            if hunter.pin.is_none() {
                let delay = hunter.voice.random(2., 3.5);
                speak(&mut hunter, &mut sounds, "JockeyZombie.Recognize", delay);
            }
        }
        _ => {
            let holding = hunter.pin.is_some();
            let busy = matches!(state.special_phase, 2 | 3);
            let alert = hunter.voice.alert;
            let mut v = std::mem::take(&mut hunter.voice);
            let chosen = idle(class, alert, holding, busy, &mut |a, b| v.random(a, b));
            hunter.voice = v;
            if let Some(chosen) = chosen {
                speak(
                    &mut hunter,
                    &mut sounds,
                    &class_voice(class, chosen.sound),
                    chosen.delay,
                );
                if let Some(timer) = chosen.timer {
                    hunter.voice.next = hunter.voice.clock + timer;
                }
            }
        }
    }
}
/// A crouched Hunter warns when a survivor is within z_pounce_silence_range
/// and inside the view cone (FUN_10347b70).
pub fn hunter_warns(eye: Vec3, forward: Vec3, squad: &Squad) -> bool {
    squad.0.iter().any(|m| {
        let delta = m.position + Vec3::Y - eye;
        !m.state.dead
            && delta.length() <= POUNCE_SILENCE_RANGE * UNIT
            && forward.dot(delta.normalize_or_zero()) >= WARN_CONE
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retail_pain_uses_short_variant_for_bullets_and_single_for_charger_tank() {
        assert_eq!(pain(SpecialClass::Hunter, true), "HunterZombie.PainShort");
        assert_eq!(pain(SpecialClass::Hunter, false), "HunterZombie.Pain");
        assert_eq!(pain(SpecialClass::Smoker, true), "SmokerZombie.PainShort");
        assert_eq!(pain(SpecialClass::Charger, true), "ChargerZombie.Pain");
        assert_eq!(pain(SpecialClass::Tank, true), "HulkZombie.Pain");
    }
    #[test]
    fn retail_death_alert_and_attack_voices_per_class() {
        assert_eq!(
            death(SpecialClass::Smoker),
            &["SmokerZombie.Death", "SmokerZombie.Explode"]
        );
        assert!(death(SpecialClass::Boomer).is_empty());
        assert_eq!(death(SpecialClass::Tank), &["HulkZombie.Die"]);
        assert_eq!(alert(SpecialClass::Boomer), Some("BoomerZombie.Rage"));
        assert_eq!(alert(SpecialClass::Tank), Some("HulkZombie.Yell"));
        assert_eq!(alert(SpecialClass::Hunter), None);
        assert_eq!(attack(SpecialClass::Hunter), Some("PlayerZombie.Attack"));
        assert_eq!(attack(SpecialClass::Charger), None);
    }
    #[test]
    fn retail_idle_voices_follow_mood_and_ability_state() {
        let mut mid = |a: f32, b: f32| (a + b) * 0.5;
        let smoker = idle(SpecialClass::Smoker, false, false, false, &mut mid).unwrap();
        assert_eq!((smoker.sound, smoker.delay), ("SmokerZombie.Breathe", 5.));
        let holding = idle(SpecialClass::Smoker, false, true, false, &mut mid).unwrap();
        assert_eq!((holding.sound, holding.delay), ("SmokerZombie.Attack", 1.5));
        let calm = idle(SpecialClass::Boomer, false, false, false, &mut mid).unwrap();
        assert_eq!((calm.sound, calm.delay), ("BoomerZombie.Groan", 2.5));
        let angry = idle(SpecialClass::Boomer, true, false, false, &mut mid).unwrap();
        assert_eq!(
            (angry.sound, angry.timer),
            ("BoomerZombie.Voice", Some(1.5))
        );
        assert!(idle(SpecialClass::Charger, false, false, true, &mut mid).is_none());
        let tank = idle(SpecialClass::Tank, true, false, false, &mut mid).unwrap();
        assert_eq!((tank.sound, tank.timer), ("HulkZombie.Yell", Some(5.)));
        let breathe = idle(SpecialClass::Tank, false, false, false, &mut |a, b| {
            if b > 50. { 80. } else { (a + b) * 0.5 }
        })
        .unwrap();
        assert_eq!(breathe.sound, "HulkZombie.Breathe");
        assert!(idle(SpecialClass::Hunter, false, false, false, &mut mid).is_none());
    }
    #[test]
    fn crouched_hunter_warns_only_for_survivors_close_and_ahead() {
        let mut member = crate::game_state::Companion {
            state: Match::new(&crate::game_state::tests_weapon()),
            position: Vec3::new(0., 0., -5.),
            spawn: Vec3::ZERO,
            model: String::new(),
            vertical: 0.,
            cooldown: 0.,
            revive_time: 0.,
            aim: Vec3::Z,
        };
        member.state.dead = false;
        let eye = Vec3::Y;
        let squad = Squad(vec![member]);
        assert!(hunter_warns(eye, Vec3::NEG_Z, &squad));
        assert!(!hunter_warns(eye, Vec3::Z, &squad));
        let mut far = squad;
        far.0[0].position = Vec3::new(0., 0., -20.);
        assert!(!hunter_warns(eye, Vec3::NEG_Z, &far));
    }
    #[test]
    fn wave_length_reads_pcm_data_and_adpcm_fact_chunks() {
        let mut pcm = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0".to_vec();
        pcm.extend_from_slice(&[1, 0, 1, 0]);
        pcm.extend_from_slice(&22050u32.to_le_bytes());
        pcm.extend_from_slice(&44100u32.to_le_bytes());
        pcm.extend_from_slice(&[2, 0, 16, 0]);
        pcm.extend_from_slice(b"data");
        pcm.extend_from_slice(&88200u32.to_le_bytes());
        pcm.resize(pcm.len() + 88200, 0);
        assert!((wave_seconds(&pcm).unwrap() - 2.).abs() < 1e-6);
        let mut adpcm = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0".to_vec();
        adpcm.extend_from_slice(&[2, 0, 1, 0]);
        adpcm.extend_from_slice(&22050u32.to_le_bytes());
        adpcm.extend_from_slice(&11100u32.to_le_bytes());
        adpcm.extend_from_slice(&[0, 2, 4, 0]);
        adpcm.extend_from_slice(b"fact\x04\0\0\0");
        adpcm.extend_from_slice(&11025u32.to_le_bytes());
        assert!((wave_seconds(&adpcm).unwrap() - 0.5).abs() < 1e-6);
    }
    #[test]
    fn retail_player_infected_sounds_exist_in_the_installed_soundscripts() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let registry = crate::source_sound::Registry::load(&mut assets).unwrap();
        let mut names = vec![
            "PlayerZombie.BecomeGhost",
            "PlayerZombie.BecomeReal",
            "PlayerZombie.AttackHit",
            "BoomerZombie.Gurgle",
            "HunterZombie.Pounce.shred",
            "HunterZombie.Warn",
            "HunterZombie.Growl",
            "JockeyZombie.Recognize",
        ];
        let steps =
            SpecialClass::ALL.map(|class| format!("{}.Concrete.RunLeft", class.step_prefix()));
        names.extend(steps.iter().map(String::as_str));
        names.push("PlayerZombie.JumpLand");
        for class in SpecialClass::ALL {
            names.push(pain(class, false));
            names.push(pain(class, true));
            names.extend(death(class));
            names.extend(alert(class));
            names.extend(attack(class));
        }
        let mut mid = |a: f32, b: f32| (a + b) * 0.5;
        for class in SpecialClass::ALL {
            for (alert, holding) in [(false, false), (true, false), (false, true)] {
                if let Some(i) = idle(class, alert, holding, false, &mut mid) {
                    names.push(i.sound);
                }
            }
        }
        for name in names {
            let def = registry
                .definitions
                .get(&name.to_ascii_lowercase())
                .unwrap_or_else(|| panic!("{name}"));
            let wave = def.waves[0].replace("$gender", "male");
            assert!(!assets.read(&wave).unwrap().is_empty(), "{name}");
        }
        // The shred slices come from the retail Melee_Pounce animation.
        let assets = std::cell::RefCell::new(assets);
        let sk = l4d2_formats::anim::Skeleton::load_with(super::super::HUNTER_MODEL, |p| {
            assets.borrow_mut().read(p).ok()
        })
        .unwrap();
        let seq = sk.find_sequence("Melee_Pounce").unwrap();
        assert!(
            sk.sequences()[seq]
                .events
                .iter()
                .any(|e| e.event == 5004 && e.options == "PlayerZombie.AttackHit")
        );
    }
}
