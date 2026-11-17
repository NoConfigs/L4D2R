//! Retail melee swings (server CTerrorMeleeWeapon): attack choice 103eaf30,
//! the swing sweep 103ebdc0 and its collision/damage test 103eb2e0, damage
//! 103ea520.
//!
//! - Primary attacks play in script order; the combo returns to the first
//!   after melee_combo_reset_time (1.2 s) without a swing.
//! - Between the attack's starttime and endtime the swing direction moves
//!   from startdir to enddir: each is +-45 degrees of yaw and/or pitch on
//!   the view. Every tick four directions spread from last tick's to this
//!   tick's are traced from the eye, melee_range (70) x the cosine to the
//!   view direction long.
//! - Each entity is damaged once per swing: common infected 175, Smoker,
//!   Boomer, Hunter, Spitter and Jockey their remaining health, Charger 65%,
//!   Witch 25% and Tank 5% of maximum health.
use super::{Body, Enemy, Rules};
use crate::collision::UNIT;
use crate::game_state::{Match, MeleeAttack, Weapon};
use crate::special_infected::SpecialClass;
use bevy::prelude::*;

/// melee_combo_reset_time.
pub(crate) const COMBO_RESET: f32 = 1.2;
/// melee_range (units).
pub(crate) const RANGE: f32 = 70.;
/// 103ea520: damage to anything that is not a player (common infected).
pub(crate) const COMMON_DAMAGE: f32 = 175.;

/// Picks the attack for a new swing (103eaf30).
pub(crate) fn next_attack(s: &mut Match, weapon: &Weapon) {
    let count = weapon.melee_attacks.len().clamp(1, 8) as u8;
    if s.melee_next != 0 && s.elapsed - s.melee_last > COMBO_RESET {
        s.melee_next = 0;
    }
    if s.melee_next >= count {
        s.melee_next = 0;
    }
    s.melee_attack = s.melee_next;
    s.melee_next += 1;
    s.melee_last = s.elapsed;
}
/// (yaw, pitch) offsets in Source degrees for a swing direction (103e9ec0,
/// 103e9f10): N S E W NE SE SW NW = 0..7; Source yaw grows to the left and
/// pitch grows downward.
pub(crate) fn offsets(direction: u8) -> (f32, f32) {
    let yaw = match direction {
        2 | 4 | 5 => -45.,
        3 | 6 | 7 => 45.,
        _ => 0.,
    };
    let pitch = match direction {
        0 | 4 | 7 => -45.,
        1 | 5 | 6 => 45.,
        _ => 0.,
    };
    (yaw, pitch)
}
/// The swing direction at `progress` (0..1) through the attack's window,
/// for the view at `yaw`/`pitch` (radians, Bevy: pitch up is positive).
pub(crate) fn direction(yaw: f32, pitch: f32, attack: &MeleeAttack, progress: f32) -> Vec3 {
    let (y0, p0) = offsets(attack.start);
    let (y1, p1) = offsets(attack.end);
    let dyaw = (y0 * (1. - progress) + y1 * progress).to_radians();
    let dpitch = -(p0 * (1. - progress) + p1 * progress).to_radians();
    Quat::from_euler(EulerRot::YXZ, yaw + dyaw, pitch + dpitch, 0.) * Vec3::NEG_Z
}
/// Damage of one melee hit (103ea520).
pub(crate) fn damage(
    health: f32,
    special: Option<SpecialClass>,
    witch: bool,
    difficulty: crate::campaign_settings::Difficulty,
) -> f32 {
    if witch {
        return crate::special_infected::witch::HEALTH * 0.25;
    }
    match special {
        Some(SpecialClass::Tank) => crate::special_infected::ai::tank_health(difficulty) * 0.05,
        Some(SpecialClass::Charger) => SpecialClass::Charger.health() * 0.65,
        Some(_) => health,
        None => COMMON_DAMAGE,
    }
}
#[derive(Default)]
pub(crate) struct Swing {
    serial: u64,
    attack: Option<MeleeAttack>,
    class: String,
    started: f32,
    previous: Option<Vec3>,
    hit: Vec<Entity>,
    world: bool,
}
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn swing(
    mut swing: Local<Swing>,
    mut s: ResMut<Match>,
    rules: Res<Rules>,
    body: Single<&Body>,
    c: Res<crate::collision::Collision>,
    mut enemies: Query<(
        Entity,
        &Transform,
        &mut Enemy,
        Option<&crate::special_infected::ai::AiSpecial>,
        Has<crate::special_infected::witch::Witch>,
    )>,
    mut sounds: Option<ResMut<crate::game_audio::Queue>>,
    mut impacts: Option<ResMut<crate::bullet_impacts::Impacts>>,
    animation: Option<Res<crate::l4d2r_animation::AnimationLibrary>>,
    playback: Option<Res<crate::l4d2r_animation::PlaybackState>>,
    session: Option<Res<crate::campaign_settings::SessionSettings>>,
    mut cinematic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
) {
    let weapon = rules.weapons.get(&s.gun().class).unwrap_or(&rules.pistol);
    if s.shot_serial != swing.serial {
        let serial = s.shot_serial;
        *swing = Swing {
            serial,
            attack: weapon
                .melee()
                .then(|| weapon.melee_attacks.get(s.melee_attack as usize).cloned())
                .flatten(),
            class: weapon.class.clone(),
            started: s.elapsed,
            ..default()
        };
    }
    let Some(attack) = swing.attack.clone() else {
        return;
    };
    if weapon.class != swing.class || s.dead || s.incap || s.infected {
        swing.attack = None;
        return;
    }
    if s.paused {
        return;
    }
    let t = s.elapsed - swing.started;
    if t > attack.end_time {
        swing.attack = None;
        return;
    }
    if t < attack.start_time {
        return;
    }
    let progress = ((t - attack.start_time) / (attack.end_time - attack.start_time).max(1e-4)).clamp(0., 1.);
    let origin = body.feet + Vec3::Y * body.stance.eye();
    let forward = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
    let now = direction(body.yaw, body.pitch, &attack, progress);
    let previous = swing.previous.unwrap_or(now);
    swing.previous = Some(now);
    let difficulty = session.as_ref().map_or(crate::campaign_settings::Difficulty::Normal, |s| s.difficulty);
    let cause = if weapon.damage_flags & 4 != 0 {
        crate::infected_death::Cause::Slash
    } else {
        crate::infected_death::Cause::Blunt
    };
    for i in 0..4 {
        let d = previous.lerp(now, i as f32 / 3.).normalize_or_zero();
        let length = RANGE * UNIT * d.dot(forward);
        if length <= 0. {
            continue;
        }
        let wall = c.trace(origin, origin + d * length, Vec3::ZERO);
        let mut nearest = wall.fraction * length;
        let mut target = None;
        for (entity, t, enemy, _, _) in &enemies {
            if enemy.health <= 0. || t.translation.distance(origin) > length + 2. {
                continue;
            }
            let boxes = animation
                .as_ref()
                .and_then(|a| a.hitboxes(&enemy.model, entity, t, playback.as_deref()));
            let hit = match &boxes {
                Some(boxes) => boxes.trace(origin, d, nearest).map(|h| (h.distance, Some((h.group, h.index)))),
                None => crate::ray_box(
                    origin,
                    d,
                    t.translation + Vec3::new(-0.35, 0., -0.35),
                    t.translation + Vec3::new(0.35, 1.8, 0.35),
                )
                .filter(|distance| *distance < nearest)
                .map(|distance| (distance, None)),
            };
            if let Some((distance, group)) = hit {
                if distance < nearest {
                    nearest = distance;
                    target = Some((entity, group));
                }
            }
        }
        let point = origin + d * nearest;
        let Some((entity, group)) = target else {
            if wall.fraction < 1. && !swing.world {
                swing.world = true;
                if let Some(logic) = cinematic.as_mut() {
                    logic.shoot_prop(origin, d, length, weapon.damage * 2., &c);
                    logic.shoot_car_alarm(origin, d, length, &c);
                    logic.shoot_breakable(
                        origin,
                        d,
                        length,
                        weapon.damage,
                        l4d2r_game_core::entities::DamageKind::Club,
                        &c,
                    );
                }
                if let Some(queue) = sounds.as_mut() {
                    if !weapon.melee_world_sound.is_empty() {
                        queue.play(&weapon.melee_world_sound, Some(point));
                    }
                }
            }
            continue;
        };
        if swing.hit.contains(&entity) {
            continue;
        }
        swing.hit.push(entity);
        let Ok((_, transform, mut enemy, special, witch)) = enemies.get_mut(entity) else {
            continue;
        };
        if !weapon.class.contains("chainsaw")
            && crate::infected_population::Kind::model(&enemy.model).blocks_front(transform, d)
        {
            continue;
        }
        if let Some(queue) = sounds.as_mut() {
            if !weapon.melee_hit_sound.is_empty() {
                queue.play(&weapon.melee_hit_sound, Some(point));
            }
        }
        if let Some(impacts) = impacts.as_mut() {
            impacts.blood.push(crate::bullet_impacts::BloodHit {
                position: point,
                direction: d,
                shotgun: false,
            });
            if let Some((_, index)) = group {
                impacts.gore.push(crate::infected_gore::Hit {
                    damage_force_source: crate::infected_gore::bullet_damage_force_source(&weapon.class, d),
                    entity,
                    position: point,
                    direction: d,
                    box_index: index,
                    distance: nearest,
                    // Melee damage types (103eb2e0): slash also dismembers.
                    damage_type: if weapon.class.contains("chainsaw") {
                        crate::infected_gore::DMG_CHAINSAW
                    } else {
                        let flags = weapon.damage_flags & (crate::infected_gore::DMG_SLASH | crate::infected_gore::DMG_CLUB);
                        crate::infected_gore::DMG_MELEE | flags
                            | if flags & crate::infected_gore::DMG_SLASH != 0 { crate::infected_gore::DMG_DISMEMBER } else { 0 }
                    },
                    sniper: false,
                    swing: Some((attack.start, attack.end)),
                    // From behind: attacker and victim yaws within 90 degrees.
                    back: {
                        let mine = Vec3::new(forward.x, 0., forward.z).normalize_or_zero();
                        let theirs = transform.rotation * Vec3::X;
                        mine.dot(Vec3::new(theirs.x, 0., theirs.z).normalize_or_zero()) > 0.
                    },
                    decapitates: weapon.decapitates,
                    seed: s.shot_serial,
                    cause,
                });
            }
        }
        enemy.health -= damage(enemy.health, special.map(|a| a.class), witch, difficulty);
        crate::game_ui::kill_feed::hit(entity, crate::game_ui::kill_feed::Killer::Player);
        enemy.reaction_serial = enemy.reaction_serial.wrapping_add(1);
        enemy.attack = animation.as_ref().map_or(0.5, |a| {
            a.infected_reaction_duration(
                &enemy.model,
                "ACT_TERROR_FLINCH",
                entity.to_bits().wrapping_add(enemy.reaction_serial.wrapping_mul(7919)),
            )
        });
        enemy.reaction = 2;
        if enemy.health <= 0. {
            enemy.record_death(cause, transform, d, point.y - transform.translation.y);
            if let Some((group, _)) = group {
                enemy.death.head = group == 1;
            }
            let seed = entity.to_bits().wrapping_add(enemy.reaction_serial.wrapping_mul(7919));
            let death = enemy.death;
            enemy.corpse_time = animation
                .as_ref()
                .map_or(5., |a| a.death_duration(&enemy.model, death, seed) + 6.);
            s.kills += 1;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn attack(start: u8, end: u8) -> MeleeAttack {
        MeleeAttack { start, end, duration: 1.23, start_time: 0.1, end_time: 0.33, activity: String::new() }
    }
    #[test]
    fn swings_sweep_from_start_to_end_direction() {
        // E -> W: from 45 degrees right to 45 degrees left of the view.
        let slash = attack(2, 3);
        let start = direction(0., 0., &slash, 0.);
        let end = direction(0., 0., &slash, 1.);
        assert!(start.x > 0.6 && end.x < -0.6, "{start:?} {end:?}");
        assert!(direction(0., 0., &slash, 0.5).abs_diff_eq(Vec3::NEG_Z, 1e-5));
        // N -> S: from 45 degrees up to 45 degrees down.
        let chop = attack(0, 1);
        assert!(direction(0., 0., &chop, 0.).y > 0.6 && direction(0., 0., &chop, 1.).y < -0.6);
    }
    #[test]
    fn melee_damage_matches_retail_targets() {
        use crate::campaign_settings::Difficulty::Normal;
        assert_eq!(damage(50., None, false, Normal), 175.);
        assert_eq!(damage(250., Some(SpecialClass::Hunter), false, Normal), 250.);
        assert_eq!(damage(37., Some(SpecialClass::Jockey), false, Normal), 37.);
        assert_eq!(damage(600., Some(SpecialClass::Charger), false, Normal), 600. * 0.65);
        assert_eq!(damage(4000., Some(SpecialClass::Tank), false, Normal), 200.);
        assert_eq!(damage(1000., None, true, Normal), 250.);
    }
    #[test]
    fn combo_alternates_and_resets_after_a_pause() {
        let mut s = Match::new(&crate::game_state::tests_weapon());
        let mut axe = crate::game_state::tests_weapon();
        axe.melee_attacks = vec![attack(2, 3), attack(3, 2)];
        s.elapsed = 1.;
        next_attack(&mut s, &axe);
        assert_eq!(s.melee_attack, 0);
        s.elapsed = 2.;
        next_attack(&mut s, &axe);
        assert_eq!(s.melee_attack, 1);
        s.elapsed = 3.;
        next_attack(&mut s, &axe);
        assert_eq!(s.melee_attack, 0, "wraps to the first attack");
        s.elapsed = 5.;
        next_attack(&mut s, &axe);
        assert_eq!(s.melee_attack, 0, "1.2 s without a swing resets the combo");
    }
}
